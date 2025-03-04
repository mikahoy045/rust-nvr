use anyhow::Result;
use futures::{SinkExt, StreamExt};
use gstreamer as gst;
use gstreamer_app as gst_app;
use gst::prelude::*;
use std::env;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::sync::broadcast;
use warp::ws::{Message, WebSocket};
use warp::Filter;
use lazy_static;

type Clients = Arc<Mutex<HashMap<String, Vec<broadcast::Sender<Vec<u8>>>>>>;

// Add this struct to hold pipeline resources
struct PipelineResources {
    pipeline: gst::Pipeline,
    _main_loop: glib::MainLoop,
}

#[tokio::main]
async fn main() -> Result<()> {
    // Load .env file
    dotenv::dotenv().ok();
    
    // Initialize GStreamer
    gst::init()?;
    
    // Get credentials
    let user = env::var("CCTV_CRED_USER").unwrap_or_else(|_| "admin".to_string());
    let pass = env::var("CCTV_CRED_PASS").unwrap_or_else(|_| "aaaa1111".to_string());
    
    // Collect all RTSP URLs from environment
    let mut rtsp_streams = HashMap::new();
    for (key, value) in env::vars() {
        if key.starts_with("CCTV_") && !key.starts_with("CCTV_CRED_") {
            rtsp_streams.insert(key, value);
        }
    }
    
    println!("Found {} RTSP streams", rtsp_streams.len());
    
    // Store clients and their broadcast channels
    let clients: Clients = Arc::new(Mutex::new(HashMap::new()));
    
    // Create a pipeline for each stream
    for (name, url) in rtsp_streams {
        println!("Setting up pipeline for {}: {}", name, url);
        
        // Create broadcast channel for this stream
        let (tx, _) = broadcast::channel(10);
        {
            let mut clients_lock = clients.lock().unwrap();
            clients_lock.insert(name.clone(), vec![tx.clone()]);
        }
        
        // Clone for closure
        let tx_clone = tx.clone();
        let stream_name = name.clone();
        let user_clone = user.clone();
        let pass_clone = pass.clone();
        
        // Setup pipeline in a separate thread
        std::thread::spawn(move || {
            if let Err(e) = setup_pipeline(&url, &user_clone, &pass_clone, tx_clone, stream_name) {
                eprintln!("Pipeline error: {:?}", e);
            }
        });
    }
    
    // Create HTML file with video elements for each stream
    create_html_file(&clients.lock().unwrap().keys().cloned().collect::<Vec<_>>())?;
    
    // Create WS handler for streams
    let clients_filter = warp::any().map(move || clients.clone());
    
    // GET /stream => HTML page
    let stream_route = warp::path("stream")
        .and(warp::get())
        .and(warp::fs::file("src/index.html"));
    
    // GET /static/... => static files
    let static_route = warp::path("static")
        .and(warp::fs::dir("static"));
    
    // GET /ws/:stream_name => websocket upgrade
    let ws_route = warp::path("ws")
        .and(warp::path::param::<String>())
        .and(warp::ws())
        .and(clients_filter)
        .map(|stream_name: String, ws: warp::ws::Ws, clients: Clients| {
            ws.on_upgrade(move |socket| handle_ws_client(socket, clients, stream_name))
        });
    
    // Combine routes
    let routes = stream_route
        .or(static_route)
        .or(ws_route);
    
    println!("Web server starting on http://localhost:3030");
    warp::serve(routes).run(([0, 0, 0, 0], 3030)).await;
    
    Ok(())
}

fn setup_pipeline(url: &str, user: &str, pass: &str, tx: broadcast::Sender<Vec<u8>>, stream_name: String) -> Result<()> {
    println!("{}: Setting up new pipeline", stream_name);
    
    // Build a much simpler pipeline
    let pipeline_str = format!(
        "rtspsrc location={} user-id={} user-pw={} ! decodebin ! videoconvert ! videoscale ! video/x-raw,width=640,height=360 ! jpegenc quality=70 ! appsink name=sink emit-signals=true sync=false",
        url, user, pass
    );
    
    println!("{}: Pipeline string: {}", stream_name, pipeline_str);
    
    // Parse and create the pipeline
    let pipeline = gst::parse::launch(&pipeline_str)?;
    let pipeline = pipeline.downcast::<gst::Pipeline>().unwrap();
    
    // Get the appsink element
    let appsink = pipeline
        .by_name("sink")
        .expect("Couldn't find appsink")
        .downcast::<gst_app::AppSink>()
        .unwrap();
    
    // Create a clone for the closure
    let stream_name_sample = stream_name.clone();
    
    // Setup appsink to collect frames
    appsink.set_callbacks(
        gst_app::AppSinkCallbacks::builder()
        .new_sample(move |app_sink| {
            let sample = match app_sink.pull_sample() {
                Ok(sample) => sample,
                Err(err) => {
                    println!("{}: Failed to pull sample: {:?}", stream_name_sample, err);
                    return Ok(gst::FlowSuccess::Ok);
                }
            };
            
            let buffer = match sample.buffer() {
                Some(buffer) => buffer,
                None => {
                    println!("{}: No buffer in sample", stream_name_sample);
                    return Ok(gst::FlowSuccess::Ok);
                }
            };
            
            let map = match buffer.map_readable() {
                Ok(map) => map,
                Err(err) => {
                    println!("{}: Failed to map buffer: {:?}", stream_name_sample, err);
                    return Ok(gst::FlowSuccess::Ok);
                }
            };
            
            // Log frame sizes
            println!("{}: Frame received - size: {} bytes", stream_name_sample, map.len());
            
            // Send the JPEG data to all connected clients
            let sent = tx.send(map.to_vec());
            println!("{}: Frame sent to {} receivers", stream_name_sample, sent.map(|r| r).unwrap_or(0));
            
            Ok(gst::FlowSuccess::Ok)
        })
        .build()
    );
    
    // Start the pipeline
    println!("{}: Setting pipeline to Playing state", stream_name);
    pipeline.set_state(gst::State::Playing)?;
    
    // Create a MainLoop but don't run it immediately
    let main_loop = glib::MainLoop::new(None, false);
    
    // Create the resources structure and keep it alive
    let resources = Arc::new(PipelineResources {
        pipeline,
        _main_loop: main_loop.clone(),
    });
    
    // Keep a global reference to resources to prevent them from being dropped
    {
        lazy_static::lazy_static! {
            static ref PIPELINES: Mutex<Vec<Arc<PipelineResources>>> = Mutex::new(Vec::new());
        }
        
        PIPELINES.lock().unwrap().push(resources.clone());
    }
    
    // Run the MainLoop in a separate thread
    std::thread::spawn(move || {
        main_loop.run();
    });
    
    Ok(())
}

async fn handle_ws_client(ws: WebSocket, clients: Clients, stream_name: String) {
    println!("New client connected to {}", stream_name);
    
    // Split the websocket
    let (mut ws_tx, mut ws_rx) = ws.split();
    
    // Find the stream name case-insensitively and get its broadcast sender
    let mut rx = {
        let clients_lock = clients.lock().unwrap();
        // Find the stream name case-insensitively
        let found_key = clients_lock.keys()
            .find(|k| k.to_lowercase() == stream_name.to_lowercase())
            .cloned();
        
        if let Some(key) = found_key {
            if let Some(senders) = clients_lock.get(&key) {
                if let Some(sender) = senders.first() {
                    println!("{}: Client successfully subscribed", key);
                    sender.subscribe()
                } else {
                    println!("{}: No senders available", key);
                    return;
                }
            } else {
                println!("{}: Stream not found (no senders)!", stream_name);
                return;
            }
        } else {
            println!("{}: Stream not found!", stream_name);
            return;
        }
    };
    
    // Handle incoming messages (mostly ping/pong)
    let incoming = tokio::spawn(async move {
        while let Some(result) = ws_rx.next().await {
            match result {
                Ok(_) => (), // Ignore client messages
                Err(_) => break, // Client disconnected
            }
        }
    });
    
    // Send frames to client
    let outgoing = tokio::spawn(async move {
        while let Ok(jpeg_data) = rx.recv().await {
            if let Err(_) = ws_tx.send(Message::binary(jpeg_data)).await {
                break; // Client disconnected
            }
        }
    });
    
    // Wait for either task to complete (client disconnect)
    tokio::select! {
        _ = incoming => {},
        _ = outgoing => {},
    }
    
    println!("Client disconnected from {}", stream_name);
}

fn create_html_file(stream_names: &[String]) -> Result<()> {
    use std::fs::File;
    use std::io::Write;
    
    let mut html = r#"
    <!DOCTYPE html>
    <html>
    <head>
        <title>RTSP Streams</title>
        <style>
            body { font-family: Arial, sans-serif; margin: 20px; background-color: #f0f0f0; }
            .container { display: flex; flex-wrap: wrap; gap: 20px; }
            .stream { background: white; border-radius: 10px; padding: 15px; box-shadow: 0 4px 6px rgba(0,0,0,0.1); }
            h1 { color: #333; }
            h2 { color: #555; margin-top: 0; }
            canvas { border: 1px solid #ddd; background: #000; display: block; }
        </style>
    </head>
    <body>
        <h1>RTSP Streams</h1>
        <div class="container">
    "#.to_string();
    
    for name in stream_names {
        html.push_str(&format!(r#"
            <div class="stream">
                <h2>{}</h2>
                <canvas id="canvas-{}" width="640" height="360"></canvas>
            </div>
        "#, name, name.to_lowercase()));
    }
    
    html.push_str(r#"
        </div>

        <script>
            function setupStream(streamName) {
                const canvas = document.getElementById('canvas-' + streamName.toLowerCase());
                const ctx = canvas.getContext('2d');
                ctx.fillStyle = 'black';
                ctx.fillRect(0, 0, canvas.width, canvas.height);
                
                // Connect to WebSocket
                const ws = new WebSocket('ws://' + window.location.host + '/ws/' + streamName.toLowerCase());
                
                ws.binaryType = 'arraybuffer';
                
                ws.onopen = function() {
                    console.log('Connected to ' + streamName);
                };
                
                ws.onmessage = function(event) {
                    const blob = new Blob([event.data], {type: 'image/jpeg'});
                    const url = URL.createObjectURL(blob);
                    const img = new Image();
                    
                    img.onload = function() {
                        ctx.drawImage(img, 0, 0, canvas.width, canvas.height);
                        URL.revokeObjectURL(url);
                    };
                    
                    img.src = url;
                };
                
                ws.onclose = function() {
                    console.log('Disconnected from ' + streamName);
                    // Try to reconnect after a delay
                    setTimeout(() => setupStream(streamName), 5000);
                };
                
                ws.onerror = function(err) {
                    console.error('Error:', err);
                    ws.close();
                };
            }
            
            // Setup all streams
    "#);
    
    for name in stream_names {
        html.push_str(&format!("            setupStream('{}');\n", name));
    }
    
    html.push_str(r#"
        </script>
    </body>
    </html>
    "#);
    
    // Create src directory if it doesn't exist
    std::fs::create_dir_all("src")?;
    
    // Write the HTML file
    let mut file = File::create("src/index.html")?;
    file.write_all(html.as_bytes())?;
    
    Ok(())
}
