// Crates
extern crate clap;
extern crate crossbeam;
extern crate framebuffer;
extern crate image;
extern crate rodio;

// Clap crate
use clap::Parser;

// Crossbeam crate
use crossbeam::channel;

// FBGL crate
use fbgl::framebuffer::*;
use fbgl::image::ImageOperations;
use fbgl::*;

// Framebuffer crate
use framebuffer::{Framebuffer, KdMode};

// Image crate
use image::imageops::FilterType;
use image::ImageReader;
use image::RgbImage;

// Rodio crate
use rodio::{source::Source, Decoder, OutputStream};

// Standard crate
use std::env::args;
use std::error::Error;
use std::fs::File;
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

fn millis() -> u128 {
    let start = SystemTime::now();
    let since_the_epoch = start
        .duration_since(UNIX_EPOCH)
        .expect("time should go forward");
    since_the_epoch.as_millis()
}

fn micros() -> u128 {
    let start = SystemTime::now();
    let since_the_epoch = start
        .duration_since(UNIX_EPOCH)
        .expect("time should go forward");
    since_the_epoch.as_millis()
}

fn start_audio(frame_dir: String) -> Result<(), String> {
    // get the output stream
    let stream_handle = match rodio::OutputStreamBuilder::open_default_stream() {
        Ok(val) => val,
        Err(err) => {
            return Err(format!(
                "Failed to open default audio stream!\nError: {:?}",
                err
            ))
        }
    };
    // get an audio sink from the stream
    let sink = rodio::Sink::connect_new(&stream_handle.mixer());
    // load the sound file
    let file = match File::open(format!("{}/music.mp3", frame_dir)) {
        Ok(val) => val,
        Err(err) => return Err(format!("Failed to open audio file!\nError: {:?}", err)),
    };
    // decode the sound file
    let source = match Decoder::try_from(file) {
        Ok(val) => val,
        Err(err) => return Err(format!("Failed to decode audio file!\nError: {:?}", err)),
    };
    // play the sound
    stream_handle.mixer().add(source);
    Ok(())
}

#[derive(Parser, Debug)]
#[command(
    name = "bad-apple-rs",
    version = "1.0",
    about = "A rust program for playing bad apple on a TFT display"
)]
struct Args {
    /// Directory to grab frames/music from
    #[arg(short, long, default_value = "/usr/share/bad-apple/")]
    directory: String,

    /// The framerate to use, default of 60 is used
    #[arg(long, default_value_t = 60)]
    framerate: usize,

    /// How many frames to preload, a zero value will use the framerate
    /// You should make sure at least a second of video is loaded continuously
    #[arg(long, default_value_t = 0)]
    preload_frames: usize,

    /// Total number of frames, for the bad apple example this was exactly 6571
    #[arg(long, default_value_t = 6571)]
    total_frames: usize,

    /// Initial delay (in milliseconds) to wait for the first round of frames to be preloaded
    /// This can be zero, but a non-zero value here lets the branch predictor to warm up
    #[arg(long, default_value_t = 500)]
    init_delay: u64,
}

fn main() {
    let args = Args::parse();
    let frame_dir = args.directory;
    println!("Using {} as frame directory!", frame_dir);

    let mut fb = Framebuffer::new("/dev/fb0").unwrap();
    let gfx_mode = Framebuffer::set_kd_mode(KdMode::Graphics);
    if !gfx_mode.is_ok() {
        println!("Failed to set graphics mode on framebuffer!");
    }

    let mut gl = BufferedRenderer::<DirectFramebufferRenderer<Color565>>::new(
        DirectFramebufferRenderer::<Color565>::new(&mut fb).unwrap(),
    );

    println!(
        "Framebuffer fb0 initialized as {}x{}!",
        gl.get_width(),
        gl.get_height()
    );

    let total_frames = args.total_frames;
    let preload_frames = if args.preload_frames > 0 {
        args.preload_frames
    } else {
        args.framerate
    };
    let (img_tx, img_rx) = channel::bounded::<RgbImage>(preload_frames);

    let scale_w = gl.get_width() as u32;
    let scale_h = gl.get_height() as u32;

    let frame_dir_clone = frame_dir.clone();
    let img_handle = thread::spawn(move || {
        let mut cur_frame = 0;
        println!("[IMG Thread]: Started!");
        'outer: while cur_frame < total_frames {
            let mut begin_frame = cur_frame;
            let (mut begin_ms, mut io_us, mut conv_us, mut decode_us) = (millis(), 0, 0, 0);
            while img_tx.len() < preload_frames && cur_frame < total_frames {
                let mut last_us;
                let img_fname = format!("{}/{:03}.jpg", frame_dir_clone, cur_frame + 1);

                // Open and read the image
                last_us = micros();
                let img_reader = match ImageReader::open(&img_fname) {
                    Ok(rdr) => rdr,
                    Err(err) => {
                        eprintln!(
                            "[IMG Thread]: Failed to load frame {} via {}",
                            cur_frame, img_fname
                        );
                        eprintln!("[IMG Thread]: Error: {:?}", err);
                        break 'outer;
                    }
                };
                io_us += micros() - last_us;

                // Decode the file contents as an image
                last_us = micros();
                let img_result = match img_reader.decode() {
                    Ok(res) => res,
                    Err(err) => {
                        eprintln!(
                            "[IMG Thread]: Failed to decode frame {} via {}",
                            cur_frame, img_fname
                        );
                        eprintln!("[IMG Thread]: Error: {:?}", err);
                        break 'outer;
                    }
                };
                decode_us += micros() - last_us;

                // Convert the image into a displayable format
                last_us = micros();
                let img_send = img_result
                    .resize_exact(scale_w, scale_h, FilterType::Nearest)
                    .to_rgb8();
                conv_us += micros() - last_us;

                img_tx
                    .send(img_send)
                    .expect("[IMG Thread]: Failed to send image through channel?!");
                cur_frame += 1;
            }
            println!(
                "[IMG Thread]: Loaded frames {} to {}, took {}ms, io {}us, decode {}us, conversion {}us",
                begin_frame,
                cur_frame,
                millis() - begin_ms, io_us, decode_us, conv_us
            );
            if img_tx.len() >= preload_frames {
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
        }
        println!("[IMG Thread]: Stopped!");
    });

    gl.clear(Color565::new(0, 0, 0));
    gl.push_buffer();
    std::thread::sleep(std::time::Duration::from_millis(args.init_delay));

    match start_audio(frame_dir) {
        Ok(_) => println!("Audio playback started!"),
        Err(str) => eprintln!("Audio playback failed! See error: {}", str),
    }

    let frametime_ms = (1000 / args.framerate) as u128;
    let mut cur_frame = 0;
    println!("[GFX Thread]: Started!");
    let mut last_ms = 0;
    while cur_frame < total_frames {
        let cur_ms = millis();
        if cur_ms > last_ms + frametime_ms {
            last_ms = cur_ms;

            // a little over 60 fps
            match img_rx.try_recv() {
                Ok(img) => {
                    //println!("[GFX Thread]: Drawing frame {}!", cur_frame);
                    gl.draw_image_rgb(0, 0, &img);
                    gl.push_buffer();
                    cur_frame += 1;
                }
                Err(crossbeam::channel::TryRecvError::Empty) => {
                    println!("[GFX Thread]: Buffer underrun, waiting 100ms to catch up!");
                }
                Err(err) => {
                    eprintln!("[GFX Thread]: Encountered unknown error with channel!");
                    eprintln!("[GFX Thread]: Error: {}", err);
                    break;
                }
            };
        } else {
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }
    println!("[GFX Thread]: Stopped!");
    img_handle.join().unwrap();

    if gfx_mode.is_ok() {
        let _ = Framebuffer::set_kd_mode(KdMode::Text);
    }
}
