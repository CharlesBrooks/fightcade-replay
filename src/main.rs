use windows::{
    Win32::Foundation::*, 
    Win32::UI::WindowsAndMessaging::*,
    Win32::UI::Input::KeyboardAndMouse::*,
};
use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::time::{Duration, Instant};
use scrap::{Capturer, Display};
use ringbuffer::{RingBuffer, AllocRingBuffer};
use std::thread;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use gstreamer as gst;
use gstreamer_app as gst_app;
use gstreamer_video as gst_video;
use gstreamer::prelude::*;

#[derive(Debug, Clone)]
struct WindowInfo {
    title: String,
    hwnd: HWND,
}

impl WindowInfo {
    fn new(hwnd: HWND) -> Option<Self> {
        unsafe {
            let length = GetWindowTextLengthW(hwnd) as usize;
            if length == 0 {
                return None;
            }

            let mut text: Vec<u16> = vec![0; length + 1];
            let chars_copied = GetWindowTextW(hwnd, &mut text);
            if chars_copied == 0 {
                return None;
            }

            text.truncate(chars_copied as usize);
            let title = OsString::from_wide(&text)
                .into_string()
                .ok()?;

            Some(WindowInfo { title, hwnd })
        }
    }

    fn contains_fightcade(&self) -> bool {
        self.title.contains("Fightcade FBNeo")
    }
}

struct GameRecorder {
    window: WindowInfo,
    buffer: AllocRingBuffer<Vec<u8>>,
    capturer: Capturer,
    frame_interval: Duration,
    should_stop: Arc<AtomicBool>,
    pipeline: Option<gst::Pipeline>,
}

impl GameRecorder {
    fn new(window: WindowInfo, buffer_seconds: u32, fps: u32) -> Result<Self, Box<dyn std::error::Error>> {
        let display = Display::primary()?;
        let capturer = Capturer::new(display)?;
        let buffer_size = (buffer_seconds * fps) as usize;
        
        // Initialize GStreamer
        gst::init()?;
        
        Ok(GameRecorder {
            window,
            buffer: AllocRingBuffer::new(buffer_size),
            capturer,
            frame_interval: Duration::from_secs(1) / fps,
            should_stop: Arc::new(AtomicBool::new(false)),
            pipeline: None,
        })
    }

    fn start_recording(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let hotkey_id = 1;
        unsafe {
            if RegisterHotKey(
                HWND(0),
                hotkey_id,
                HOT_KEY_MODIFIERS(MOD_CONTROL.0),
                0x52, // Virtual key code for 'R'
            ).as_bool()
            {
                println!("Successfully registered Ctrl+R hotkey");
            } else {
                println!("Failed to register hotkey: {}", std::io::Error::last_os_error());
            }
        }

        let start_time = Instant::now();
        println!("Recording started. Press Ctrl+R to save last 15 seconds.");
        println!("Press Ctrl+C to exit.");

        let mut msg = MSG::default();

        while !self.should_stop.load(Ordering::Relaxed) {
            unsafe {
                if PeekMessageW(&mut msg, HWND(0), 0, 0, PM_REMOVE).as_bool() {
                    if msg.message == WM_HOTKEY && msg.wParam.0 == hotkey_id as usize {
                        println!("Hotkey pressed!");
                        self.save_buffer("replay.mp4")?;
                    }
                }
            }

            if let Ok(frame) = self.capturer.frame() {
                self.buffer.push(frame.to_vec());
            }

            let elapsed = start_time.elapsed();
            if elapsed < self.frame_interval {
                thread::sleep(self.frame_interval - elapsed);
            }
        }

        unsafe {
            UnregisterHotKey(HWND(0), hotkey_id);
        }

        Ok(())
    }

    fn save_buffer(&self, output_path: &str) -> Result<(), Box<dyn std::error::Error>> {
        // Create a simpler pipeline using the basic H264 encoder
        let pipeline_str = format!(
            "appsrc name=source do-timestamp=true ! videoconvert ! video/x-raw,format=BGRx ! avenc_h264 ! h264parse ! mp4mux ! filesink location={}",
            output_path
        );
        
        let pipeline = gst::parse_launch(&pipeline_str)?;
        let pipeline = pipeline.dynamic_cast::<gst::Pipeline>().unwrap();
        
        let appsrc = pipeline
            .by_name("source")
            .unwrap()
            .dynamic_cast::<gst_app::AppSrc>()
            .unwrap();
        
        // Configure video format
        let video_info = gst_video::VideoInfo::builder(
            gst_video::VideoFormat::Bgrx,  // Changed to BGRx to match the pipeline
            1920,
            1080
        )
        .fps(gst::Fraction::new(30, 1))
        .build()
        .unwrap();
        
        appsrc.set_caps(Some(&video_info.to_caps().unwrap()));
        appsrc.set_format(gst::Format::Time);
        
        pipeline.set_state(gst::State::Playing)?;
        
        // Push frames with proper timing
        let frame_duration = gst::ClockTime::from_nseconds(33333333); // ~30fps
        let mut pts = gst::ClockTime::ZERO;
        
        for frame_data in self.buffer.iter() {
            let mut buffer = gst::Buffer::from_slice(frame_data.clone());
            {
                let buffer = buffer.get_mut().unwrap();
                buffer.set_pts(pts);
                buffer.set_duration(frame_duration);
            }
            pts += frame_duration;
            
            appsrc.push_buffer(buffer)?;
        }
        
        // Proper cleanup
        appsrc.end_of_stream()?;
        
        // Wait for pipeline to finish
        let bus = pipeline.bus().unwrap();
        for msg in bus.iter_timed(gst::ClockTime::NONE) {
            use gst::MessageView;
            match msg.view() {
                MessageView::Eos(..) => break,
                MessageView::Error(err) => {
                    pipeline.set_state(gst::State::Null)?;
                    return Err(Box::new(std::io::Error::new(
                        std::io::ErrorKind::Other,
                        format!("Error: {:?}", err)
                    )));
                }
                _ => (),
            }
        }
        
        pipeline.set_state(gst::State::Null)?;
        println!("Replay saved to {}", output_path);
        Ok(())
    }
}

struct WindowEnumerator {
    windows: Vec<WindowInfo>,
}

impl WindowEnumerator {
    fn new() -> Self {
        WindowEnumerator {
            windows: Vec::new(),
        }
    }

    fn enumerate(mut self) -> Vec<WindowInfo> {
        unsafe {
            let result = EnumWindows(
                Some(Self::enum_window_callback),
                LPARAM(&mut self.windows as *mut _ as isize),
            );
            if !result.as_bool() {
                eprintln!("Failed to enumerate windows");
            }
        }
        self.windows
    }

    unsafe extern "system" fn enum_window_callback(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let windows = &mut *(lparam.0 as *mut Vec<WindowInfo>);
        
        if let Some(info) = WindowInfo::new(hwnd) {
            if info.contains_fightcade() {
                windows.push(info);
            }
        }
        
        true.into()
    }
}

fn get_fightcade_windows() -> Vec<WindowInfo> {
    WindowEnumerator::new().enumerate()
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let windows = get_fightcade_windows();
    
    if let Some(window) = windows.first() {
        println!("Found Fightcade window: {}", window.title);
        
        // Setup Ctrl+C handler
        let should_stop = Arc::new(AtomicBool::new(false));
        let should_stop_clone = should_stop.clone();
        ctrlc::set_handler(move || {
            println!("Stopping recording...");
            should_stop_clone.store(true, Ordering::Relaxed);
        })?;

        let mut recorder = GameRecorder::new(window.clone(), 15, 30)?;
        recorder.should_stop = should_stop;
        recorder.start_recording()?;
    } else {
        println!("No Fightcade windows found!");
    }

    Ok(())
}
