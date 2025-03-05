use anyhow::Result;
// use futures::{SinkExt, StreamExt};
use gstreamer as gst;
use gstreamer_app as gst_app;
use gst::prelude::*;
use std::sync::{Arc, Mutex};
use tokio::sync::broadcast;
use lazy_static::lazy_static;
// use gst::glib::ControlFlow;

pub struct PipelineResources {
    #[allow(dead_code)]
    pub pipeline: gst::Pipeline,
    pub _main_loop: glib::MainLoop,
}

pub fn setup_pipeline(
    url: &str,
    user: &str,
    pass: &str,
    tx: broadcast::Sender<Vec<u8>>,
    stream_name: String,
) -> Result<()> {
    println!("{}: Setting up new pipeline", stream_name);
    
    let pipeline_str = format!(
        "rtspsrc location={} user-id={} user-pw={} ! decodebin ! videoconvert ! videoscale ! video/x-raw,width=640,height=360 ! jpegenc quality=70 ! appsink name=sink emit-signals=true sync=false",
        url, user, pass
    );
    
    println!("{}: Pipeline string: {}", stream_name, pipeline_str);
    
    let pipeline = gst::parse::launch(&pipeline_str)?;
    let pipeline = pipeline.downcast::<gst::Pipeline>().unwrap();
    
    let appsink = pipeline
        .by_name("sink")
        .expect("Couldn't find appsink")
        .downcast::<gst_app::AppSink>()
        .unwrap();
    
    let stream_name_sample = stream_name.clone();
    // let stream_name_bus = stream_name.clone();
    
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
                
                println!("{}: Frame received - size: {} bytes", stream_name_sample, map.len());
                let sent = tx.send(map.to_vec());
                println!("{}: Frame sent to {} receivers", stream_name_sample, sent.map(|r| r).unwrap_or(0));
                
                Ok(gst::FlowSuccess::Ok)
            })
            .build()
    );
    
    println!("{}: Setting pipeline to Playing state", stream_name);
    pipeline.set_state(gst::State::Playing)?;
    
    // let bus = pipeline.bus().unwrap();
    // bus.add_watch(move |_, msg| {
    //     println!("{}: Bus message: {:?}", stream_name_bus, msg.view());
    //     ControlFlow::Continue
    // }).expect("Failed to add bus watch");
    
    let main_loop = glib::MainLoop::new(None, false);
    
    let resources = Arc::new(PipelineResources {
        pipeline,
        _main_loop: main_loop.clone(),
    });
    
    lazy_static! {
        static ref PIPELINES: Mutex<Vec<Arc<PipelineResources>>> = Mutex::new(Vec::new());
    }
    
    PIPELINES.lock().unwrap().push(resources.clone());
    
    std::thread::spawn(move || {
        main_loop.run();
    });
    
    Ok(())
} 