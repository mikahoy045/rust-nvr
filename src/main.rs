use anyhow::Result;
// use futures::{SinkExt, StreamExt};
use gstreamer as gst;
// use gstreamer_app as gst_app;
// use gst::prelude::*;
use std::env;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::sync::broadcast;
// use warp::ws::{Message, WebSocket};
use warp::Filter;
// use lazy_static;

mod rtsp;
mod web;

pub type Clients = Arc<Mutex<HashMap<String, Vec<broadcast::Sender<Vec<u8>>>>>>;

// Add this struct to hold pipeline resources
// struct PipelineResources {
//     #[allow(dead_code)]
//     pipeline: gst::Pipeline,
//     _main_loop: glib::MainLoop,
// }

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
            if let Err(e) = rtsp::setup_pipeline(&url, &user_clone, &pass_clone, tx_clone, stream_name) {
                eprintln!("Pipeline error: {:?}", e);
            }
        });
    }
    
    // Create HTML file with video elements for each stream
    web::create_html_file(&clients.lock().unwrap().keys().cloned().collect::<Vec<_>>())?;
    
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
            ws.on_upgrade(move |socket| web::handle_ws_client(socket, clients, stream_name))
        });
    
    // Combine routes
    let routes = stream_route
        .or(static_route)
        .or(ws_route);
    
    println!("Web server starting on http://localhost:3030");
    warp::serve(routes).run(([0, 0, 0, 0], 3030)).await;
    
    Ok(())
}
