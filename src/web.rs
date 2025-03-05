use anyhow::Result;
use futures::{SinkExt, StreamExt};
// use std::sync::Mutex;
// use std::sync::Arc;
use warp::ws::{Message, WebSocket};
// use tokio::sync::broadcast;

pub async fn handle_ws_client(ws: WebSocket, clients: crate::Clients, stream_name: String) {
    println!("New client connected to {}", stream_name);
    
    let (mut ws_tx, mut ws_rx) = ws.split();
    
    let mut rx = {
        let clients_lock = clients.lock().unwrap();
        println!("Available streams: {:?}", clients_lock.keys().collect::<Vec<_>>());
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
    
    let _ = ws_tx.send(Message::text("Connected to stream")).await;
    
    let incoming = tokio::spawn(async move {
        while let Some(result) = ws_rx.next().await {
            if result.is_err() {
                break;
            }
        }
    });
    
    let outgoing = tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(jpeg_data) => {
                    println!("Sending frame of size {} to client", jpeg_data.len());
                    if ws_tx.send(Message::binary(jpeg_data)).await.is_err() {
                        break;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    println!("Dropped {} messages", n);
                    continue;
                }
                Err(_) => break,
            }
        }
    });
    
    tokio::select! {
        _ = incoming => println!("Incoming task completed"),
        _ = outgoing => println!("Outgoing task completed"),
    }
    
    println!("Client disconnected from {}", stream_name);
}

pub fn create_html_file(stream_names: &[String]) -> Result<()> {
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
                position: fixed;
                bottom: 0;
                left: 0;
                right: 0;
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
                display: flex;
                align-items: center;
                gap: 5px;
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
            svg {
                width: 16px;
                height: 16px;
                fill: currentColor;
            }
            .control-btn svg {
                width: 20px;
                height: 20px;
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
                    <div class="control-btn">
                        <svg viewBox="0 0 24 24">
                            <path d="M17.65,6.35C16.2,4.9 14.21,4 12,4A8,8 0 0,0 4,12A8,8 0 0,0 12,20C15.73,20 18.84,17.45 19.73,14H17.65C16.83,16.33 14.61,18 12,18A6,6 0 0,1 6,12A6,6 0 0,1 12,6C13.66,6 15.14,6.69 16.22,7.78L13,11H20V4L17.65,6.35Z" />
                        </svg>
                    </div>
                    <div class="control-btn">
                        <svg viewBox="0 0 24 24">
                            <path d="M14,19H18V5H14M6,19H10V5H6V19Z" />
                        </svg>
                    </div>
                    <div class="control-btn">
                        <svg viewBox="0 0 24 24">
                            <path d="M12,15.5A3.5,3.5 0 0,1 8.5,12A3.5,3.5 0 0,1 12,8.5A3.5,3.5 0 0,1 15.5,12A3.5,3.5 0 0,1 12,15.5M19.43,12.97C19.47,12.65 19.5,12.33 19.5,12C19.5,11.67 19.47,11.34 19.43,11L21.54,9.37C21.73,9.22 21.78,8.95 21.66,8.73L19.66,5.27C19.54,5.05 19.27,4.96 19.05,5.05L16.56,6.05C16.04,5.66 15.5,5.32 14.87,5.07L14.5,2.42C14.46,2.18 14.25,2 14,2H10C9.75,2 9.54,2.18 9.5,2.42L9.13,5.07C8.5,5.32 7.96,5.66 7.44,6.05L4.95,5.05C4.73,4.96 4.46,5.05 4.34,5.27L2.34,8.73C2.21,8.95 2.27,9.22 2.46,9.37L4.57,11C4.53,11.34 4.5,11.67 4.5,12C4.5,12.33 4.53,12.65 4.57,12.97L2.46,14.63C2.27,14.78 2.21,15.05 2.34,15.27L4.34,18.73C4.46,18.95 4.73,19.03 4.95,18.95L7.44,17.94C7.96,18.34 8.5,18.68 9.13,18.93L9.5,21.58C9.54,21.82 9.75,22 10,22H14C14.25,22 14.46,21.82 14.5,21.58L14.87,18.93C15.5,18.67 16.04,18.34 16.56,17.94L19.05,18.95C19.27,19.03 19.54,18.95 19.66,18.73L21.66,15.27C21.78,15.05 21.73,14.78 21.54,14.63L19.43,12.97Z" />
                        </svg>
                    </div>
                </div>
                <div class="stats" id="stats-{}"></div>
            </div>
        "#, name, name.to_lowercase(), name.to_lowercase(), name, name.to_lowercase()));
    }
    
    html.push_str(r#"
        </div>
        <div class="toolbar">
            <button class="toolbar-btn">
                <svg viewBox="0 0 24 24">
                    <path d="M12,20A8,8 0 0,1 4,12A8,8 0 0,1 12,4A8,8 0 0,1 20,12A8,8 0 0,1 12,20M12,2A10,10 0 0,0 2,12A10,10 0 0,0 12,22A10,10 0 0,0 22,12A10,10 0 0,0 12,2M12,12.5A1.5,1.5 0 0,1 10.5,11A1.5,1.5 0 0,1 12,9.5A1.5,1.5 0 0,1 13.5,11A1.5,1.5 0 0,1 12,12.5M12,7.2C9.9,7.2 8.2,8.9 8.2,11C8.2,14 12,17.5 12,17.5C12,17.5 15.8,14 15.8,11C15.8,8.9 14.1,7.2 12,7.2Z" />
                </svg>
                Record
            </button>
            <button class="toolbar-btn">
                <svg viewBox="0 0 24 24">
                    <path d="M4,4H7L9,2H15L17,4H20A2,2 0 0,1 22,6V18A2,2 0 0,1 20,20H4A2,2 0 0,1 2,18V6A2,2 0 0,1 4,4M12,7A5,5 0 0,0 7,12A5,5 0 0,0 12,17A5,5 0 0,0 17,12A5,5 0 0,0 12,7M12,9A3,3 0 0,1 15,12A3,3 0 0,1 12,15A3,3 0 0,1 9,12A3,3 0 0,1 12,9Z" />
                </svg>
                Snapshot
            </button>
            <button class="toolbar-btn">
                <svg viewBox="0 0 24 24">
                    <path d="M12,15.5A3.5,3.5 0 0,1 8.5,12A3.5,3.5 0 0,1 12,8.5A3.5,3.5 0 0,1 15.5,12A3.5,3.5 0 0,1 12,15.5M19.43,12.97C19.47,12.65 19.5,12.33 19.5,12C19.5,11.67 19.47,11.34 19.43,11L21.54,9.37C21.73,9.22 21.78,8.95 21.66,8.73L19.66,5.27C19.54,5.05 19.27,4.96 19.05,5.05L16.56,6.05C16.04,5.66 15.5,5.32 14.87,5.07L14.5,2.42C14.46,2.18 14.25,2 14,2H10C9.75,2 9.54,2.18 9.5,2.42L9.13,5.07C8.5,5.32 7.96,5.66 7.44,6.05L4.95,5.05C4.73,4.96 4.46,5.05 4.34,5.27L2.34,8.73C2.21,8.95 2.27,9.22 2.46,9.37L4.57,11C4.53,11.34 4.5,11.67 4.5,12C4.5,12.33 4.53,12.65 4.57,12.97L2.46,14.63C2.27,14.78 2.21,15.05 2.34,15.27L4.34,18.73C4.46,18.95 4.73,19.03 4.95,18.95L7.44,17.94C7.96,18.34 8.5,18.68 9.13,18.93L9.5,21.58C9.54,21.82 9.75,22 10,22H14C14.25,22 14.46,21.82 14.5,21.58L14.87,18.93C15.5,18.67 16.04,18.34 16.56,17.94L19.05,18.95C19.27,19.03 19.54,18.95 19.66,18.73L21.66,15.27C21.78,15.05 21.73,14.78 21.54,14.63L19.43,12.97Z" />
                </svg>
                Settings
            </button>
            <button class="toolbar-btn" id="fullscreen-btn">
                <svg viewBox="0 0 24 24">
                    <path d="M5,5H10V7H7V10H5V5M14,5H19V10H17V7H14V5M17,14H19V19H14V17H17V14M10,17V19H5V14H7V17H10Z" />
                </svg>
                Full Screen
            </button>
            <button class="toolbar-btn">
                <svg viewBox="0 0 24 24">
                    <path d="M9.5,3A6.5,6.5 0 0,1 16,9.5C16,11.11 15.41,12.59 14.44,13.73L14.71,14H15.5L20.5,19L19,20.5L14,15.5V14.71L13.73,14.44C12.59,15.41 11.11,16 9.5,16A6.5,6.5 0 0,1 3,9.5A6,6,0 0,1 9.5,3M9.5,5C7,5 5,7 5,9.5C5,12 7,14 9.5,14C12,14 14,12 14,9.5C14,7 12,5 9.5,5Z" />
                </svg>
                Search
            </button>
        </div>
    
        <script>
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
                
                ctx.fillStyle = 'white';
                ctx.font = '16px Arial';
                ctx.textAlign = 'center';
                ctx.fillText('Connecting to ' + streamName + '...', canvas.width/2, canvas.height/2);
                
                let frameCount = 0;
                let lastTime = Date.now();
                let fps = 0;
                
                const ws = new WebSocket('ws://' + window.location.host + '/ws/' + streamName.toLowerCase());
                
                ws.binaryType = 'arraybuffer';
                
                ws.onopen = function() {
                    console.log('Connected to ' + streamName);
                    stats.textContent = 'Connected';
                    statusDot.style.backgroundColor = '#4CAF50';
                };
                
                ws.onmessage = function(event) {
                    frameCount++;
                    const now = Date.now();
                    if (now - lastTime >= 1000) {
                        fps = frameCount;
                        frameCount = 0;
                        lastTime = now;
                        fpsElement.textContent = fps + ' FPS';
                    }
                    
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
                    statusDot.style.backgroundColor = '#FF9800';
                    
                    ctx.fillStyle = 'black';
                    ctx.fillRect(0, 0, canvas.width, canvas.height);
                    ctx.fillStyle = 'red';
                    ctx.font = '16px Arial';
                    ctx.textAlign = 'center';
                    ctx.fillText('Connection lost. Reconnecting...', canvas.width/2, canvas.height/2);
                    
                    setTimeout(() => setupStream(streamName), 5000);
                };
                
                ws.onerror = function(err) {
                    console.error('WebSocket Error for ' + streamName + ':', err);
                    statusDot.style.backgroundColor = 'red';
                };
                
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
            
    "#);
    
    for name in stream_names {
        html.push_str(&format!("            setupStream('{}');\n", name));
    }
    
    html.push_str(r#"
            document.getElementById('fullscreen-btn').addEventListener('click', function() {
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
    
    std::fs::create_dir_all("src")?;
    let mut file = File::create("src/index.html")?;
    file.write_all(html.as_bytes())?;
    
    Ok(())
} 