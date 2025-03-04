use anyhow::Result;
use gstreamer as gst;
use gst::prelude::*;
use std::env;
use std::collections::HashMap;

fn main() -> Result<()> {
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
    
    // Create a pipeline for each stream
    let mut pipelines = Vec::new();
    
    for (name, url) in rtsp_streams {
        println!("Setting up pipeline for {}: {}", name, url);
        
        // Create the elements
        let source = gst::ElementFactory::make("rtspsrc")
            .property("location", &url)
            .property("user-id", &user)
            .property("user-pw", &pass)
            .property("latency", 0u32)
            .build()?;
        
        // Creating a simpler pipeline with decodebin to handle any codec
        let decode_bin = gst::ElementFactory::make("decodebin").build()?;
        let convert = gst::ElementFactory::make("videoconvert").build()?;
        
        // Create a unique sink name for each stream
        let sink_name = format!("autovideosink-{}", name);
        let sink = gst::ElementFactory::make("autovideosink")
            .name(&sink_name)
            .build()?;
        
        // Create the pipeline
        let pipeline = gst::Pipeline::new();
        
        // Add all elements to the pipeline
        pipeline.add_many([&source, &decode_bin, &convert, &sink])?;
        
        // Link elements that can be linked statically
        gst::Element::link_many([&convert, &sink])?;
        
        // Create a clone of convert for the decodebin closure
        let convert_clone = convert.clone();
        let _pipeline_clone = pipeline.clone();
        
        // Connect pad-added signal for dynamic linking from decodebin to converter
        decode_bin.connect_pad_added(move |_, src_pad| {
            let sink_pad = convert_clone.static_pad("sink").expect("Failed to get static sink pad from videoconvert");
            
            if !src_pad.is_linked() {
                println!("Linking decode_bin src_pad to videoconvert sink_pad");
                
                // Only attempt linking if the pad is compatible
                if src_pad.can_link(&sink_pad) {
                    match src_pad.link(&sink_pad) {
                        Ok(_) => println!("Link successful"),
                        Err(err) => println!("Link failed: {:?}", err),
                    }
                } else {
                    println!("Pads not compatible for linking");
                }
            }
        });
        
        // Create a clone of decode_bin for the rtspsrc closure
        let decode_bin_clone = decode_bin.clone();
        
        // Connect pad-added signal for dynamic linking from rtspsrc to decodebin
        source.connect_pad_added(move |_, src_pad| {
            println!("Pad added from rtspsrc: {:?}", src_pad);
            
            let caps = src_pad.current_caps().expect("Failed to get caps");
            println!("Pad caps: {:?}", caps);
            
            // Get the static sink pad from decodebin instead of requesting one
            let sink_pad = decode_bin_clone.static_pad("sink").expect("Failed to get static sink pad from decodebin");
            
            if !src_pad.is_linked() {
                println!("Linking rtspsrc src_pad to decodebin sink_pad");
                
                match src_pad.link(&sink_pad) {
                    Ok(_) => println!("Link successful"),
                    Err(err) => println!("Link failed: {:?}", err),
                }
            }
        });
        
        // Start playing
        pipeline.set_state(gst::State::Playing)?;
        
        // Save the pipeline
        pipelines.push((name, pipeline));
    }
    
    if pipelines.is_empty() {
        println!("No RTSP streams found in .env file!");
        return Ok(());
    }
    
    println!("Playing {} RTSP streams. Press Ctrl+C to stop.", pipelines.len());
    
    // Create a mainloop to listen for Ctrl+C
    let main_loop = glib::MainLoop::new(None, false);
    let main_loop_clone = main_loop.clone();
    
    ctrlc::set_handler(move || {
        println!("\nReceived Ctrl+C, stopping streams...");
        main_loop_clone.quit();
    })
    .expect("Error setting Ctrl+C handler");
    
    // Run the main loop
    main_loop.run();
    
    // Clean up
    for (name, pipeline) in pipelines {
        println!("Stopping stream: {}", name);
        pipeline.set_state(gst::State::Null)?;
    }
    
    println!("All streams stopped");
    
    Ok(())
}
