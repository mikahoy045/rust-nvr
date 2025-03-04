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
    #[allow(dead_code)]
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
        
        // Create broadcast channel for this stream with larger buffer
        let (tx, _) = broadcast::channel(100); // Increase buffer size
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
        
        // Print all available stream names for debugging
        println!("Available streams: {:?}", clients_lock.keys().collect::<Vec<_>>());
        
        // Find the stream name case-insensitively
        let found_key = clients_lock.keys()
            .find(|k| k.to_lowercase() == stream_name.to_lowercase())
            .cloned();
        
        if let Some(key) = found_key {
            println!("Found matching stream: {}", key);
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
            println!("{}: Stream not found! Available: {:?}", 
                stream_name, 
                clients_lock.keys().collect::<Vec<_>>());
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
            println!("Sending frame of size {} to client", jpeg_data.len());
            if let Err(_) = ws_tx.send(Message::binary(jpeg_data)).await {
                break; // Client disconnected
            }
        }
    });
    
    // Wait for either task to complete (client disconnect)
    tokio::select! {
        _ = incoming => println!("Incoming task completed"),
        _ = outgoing => println!("Outgoing task completed"),
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
        <title>CCTV Surveillance System</title>
        <meta name="viewport" content="width=device-width, initial-scale=1.0">
        <style>
            body {
                font-family: 'Segoe UI', Tahoma, Geneva, Verdana, sans-serif;
                margin: 0;
                padding: 0;
                background-color: #1e1e1e;
                color: #e0e0e0;
                overflow: hidden;
            }
            .header {
                background-color: #333;
                color: white;
                padding: 10px 20px;
                display: flex;
                justify-content: space-between;
                align-items: center;
                border-bottom: 1px solid #444;
            }
            .header h1 {
                margin: 0;
                font-size: 18px;
                font-weight: 500;
            }
            .datetime {
                font-size: 14px;
                text-align: right;
            }
            .container {
                display: grid;
                grid-template-columns: repeat(3, 1fr);
                gap: 8px;
                padding: 8px;
                height: calc(100vh - 60px);
            }
            .stream {
                background: #2a2a2a;
                border-radius: 4px;
                overflow: hidden;
                position: relative;
                box-shadow: 0 2px 4px rgba(0,0,0,0.3);
            }
            .stream-header {
                background: rgba(0,0,0,0.7);
                color: white;
                padding: 5px 10px;
                position: absolute;
                top: 0;
                left: 0;
                right: 0;
                z-index: 10;
                display: flex;
                justify-content: space-between;
                font-size: 12px;
            }
            .stream-name {
                font-weight: bold;
            }
            .status {
                display: flex;
                align-items: center;
            }
            .status-dot {
                height: 8px;
                width: 8px;
                border-radius: 50%;
                background-color: #4CAF50;
                margin-right: 5px;
            }
            .status-text {
                font-size: 11px;
            }
            canvas {
                width: 100%;
                height: 100%;
                background: #000;
                display: block;
                object-fit: cover;
            }
            .stream-footer {
                background: rgba(0,0,0,0.7);
                color: white;
                padding: 5px 10px;
                position: absolute;
                bottom: 0;
                left: 0;
                right: 0;
                z-index: 10;
                display: flex;
                justify-content: space-between;
                font-size: 11px;
            }
            .controls {
                position: absolute;
                top: 50%;
                left: 50%;
                transform: translate(-50%, -50%);
                display: flex;
                gap: 10px;
                opacity: 0;
                transition: opacity 0.3s;
                z-index: 5;
            }
            .stream:hover .controls {
                opacity: 1;
            }
            .control-btn {
                width: 36px;
                height: 36px;
                border-radius: 50%;
                background: rgba(0,0,0,0.7);
                border: 1px solid rgba(255,255,255,0.3);
                color: white;
                display: flex;
                align-items: center;
                justify-content: center;
                cursor: pointer;
            }
            .control-btn:hover {
                background: rgba(0,0,0,0.9);
            }
            .toolbar {
                background: #333;
                padding: 5px 10px;
                display: flex;
                justify-content: center;
                gap: 20px;
                border-top: 1px solid #444;
            }
            .toolbar-btn {
                background: transparent;
                border: none;
                color: #ddd;
                cursor: pointer;
                padding: 5px 10px;
                font-size: 13px;
            }
            .toolbar-btn:hover {
                color: white;
                background: #444;
                border-radius: 3px;
            }
            .stats {
                position: absolute;
                bottom: 25px;
                right: 10px;
                background: rgba(0,0,0,0.5);
                color: #aaa;
                font-size: 10px;
                padding: 2px 5px;
                border-radius: 3px;
                z-index: 15;
            }
        </style>
    </head>
    <body>
        <div class="header">
            <h1>CCTV Surveillance System</h1>
            <div class="datetime" id="datetime">Loading...</div>
        </div>
        <div class="container">
    "#.to_string();
    
    for name in stream_names {
        html.push_str(&format!(r#"
            <div class="stream">
                <div class="stream-header">
                    <div class="stream-name">{}</div>
                    <div class="status">
                        <div class="status-dot"></div>
                        <div class="status-text">LIVE</div>
                    </div>
                </div>
                <canvas id="canvas-{}" width="640" height="360"></canvas>
                <div class="stream-footer">
                    <div class="fps" id="fps-{}">0 FPS</div>
                    <div class="location">{}</div>
                </div>
                <div class="controls">
                    <div class="control-btn">⟲</div>
                    <div class="control-btn">⏸</div>
                    <div class="control-btn">⚙</div>
                </div>
                <div class="stats" id="stats-{}"></div>
            </div>
        "#, name, name.to_lowercase(), name.to_lowercase(), name, name.to_lowercase()));
    }
    
    html.push_str(r#"
        </div>
        <div class="toolbar">
            <button class="toolbar-btn">⏺ Record</button>
            <button class="toolbar-btn">📷 Snapshot</button>
            <button class="toolbar-btn">⚙ Settings</button>
            <button class="toolbar-btn">⤢ Full Screen</button>
            <button class="toolbar-btn">🔍 Search</button>
        </div>

        <script>
            // Update date and time
            function updateDateTime() {
                const now = new Date();
                const dateString = now.toLocaleDateString();
                const timeString = now.toLocaleTimeString();
                document.getElementById('datetime').textContent = `${dateString} ${timeString}`;
            }
            
            setInterval(updateDateTime, 1000);
            updateDateTime();
            
            function setupStream(streamName) {
                const canvas = document.getElementById('canvas-' + streamName.toLowerCase());
                const ctx = canvas.getContext('2d');
                const stats = document.getElementById('stats-' + streamName.toLowerCase());
                const fpsElement = document.getElementById('fps-' + streamName.toLowerCase());
                const statusDot = canvas.parentElement.querySelector('.status-dot');
                
                ctx.fillStyle = 'black';
                ctx.fillRect(0, 0, canvas.width, canvas.height);
                
                // Draw text on canvas
                ctx.fillStyle = 'white';
                ctx.font = '16px Arial';
                ctx.textAlign = 'center';
                ctx.fillText('Connecting to ' + streamName + '...', canvas.width/2, canvas.height/2);
                
                let frameCount = 0;
                let lastTime = Date.now();
                let fps = 0;
                
                // Connect to WebSocket
                const ws = new WebSocket('ws://' + window.location.host + '/ws/' + streamName.toLowerCase());
                
                ws.binaryType = 'arraybuffer';
                
                ws.onopen = function() {
                    console.log('Connected to ' + streamName);
                    stats.textContent = 'Connected';
                    statusDot.style.backgroundColor = '#4CAF50'; // Green
                };
                
                ws.onmessage = function(event) {
                    // Calculate FPS
                    frameCount++;
                    const now = Date.now();
                    if (now - lastTime >= 1000) {
                        fps = frameCount;
                        frameCount = 0;
                        lastTime = now;
                        fpsElement.textContent = fps + ' FPS';
                    }
                    
                    // Update stats
                    stats.textContent = `${(event.data.byteLength / 1024).toFixed(1)} KB`;
                    
                    const blob = new Blob([event.data], {type: 'image/jpeg'});
                    const url = URL.createObjectURL(blob);
                    const img = new Image();
                    
                    img.onload = function() {
                        ctx.drawImage(img, 0, 0, canvas.width, canvas.height);
                        URL.revokeObjectURL(url);
                    };
                    
                    img.onerror = function(err) {
                        console.error(`Error loading image for ${streamName}:`, err);
                        statusDot.style.backgroundColor = 'red';
                    };
                    
                    img.src = url;
                };
                
                ws.onclose = function() {
                    console.log('Disconnected from ' + streamName);
                    statusDot.style.backgroundColor = '#FF9800'; // Orange
                    
                    // Draw text on canvas
                    ctx.fillStyle = 'black';
                    ctx.fillRect(0, 0, canvas.width, canvas.height);
                    ctx.fillStyle = 'red';
                    ctx.font = '16px Arial';
                    ctx.textAlign = 'center';
                    ctx.fillText('Connection lost. Reconnecting...', canvas.width/2, canvas.height/2);
                    
                    // Try to reconnect after a delay
                    setTimeout(() => setupStream(streamName), 5000);
                };
                
                ws.onerror = function(err) {
                    console.error('WebSocket Error for ' + streamName + ':', err);
                    statusDot.style.backgroundColor = 'red';
                };
                
                // Fullscreen toggle
                canvas.addEventListener('dblclick', function() {
                    if (!document.fullscreenElement) {
                        canvas.parentElement.requestFullscreen().catch(err => {
                            console.error(`Could not enter fullscreen: ${err.message}`);
                        });
                    } else {
                        document.exitFullscreen();
                    }
                });
            }
            
            // Setup all streams
    "#);
    
    for name in stream_names {
        html.push_str(&format!("            setupStream('{}');\n", name));
    }
    
    html.push_str(r#"
            // Toolbar buttons
            document.querySelector('.toolbar-btn:nth-child(4)').addEventListener('click', function() {
                if (!document.fullscreenElement) {
                    document.documentElement.requestFullscreen().catch(err => {
                        console.error(`Could not enter fullscreen: ${err.message}`);
                    });
                } else {
                    document.exitFullscreen();
                }
            });
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
