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
use rodio::{Decoder, OutputStream, Sink};

// Standard crate
use std::fs::File;
use std::thread;
use std::thread::JoinHandle;
use std::time::{SystemTime, UNIX_EPOCH};
use std::vec::Vec;

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
    since_the_epoch.as_micros()
}

struct ImageThreadOptions {
    disp_w: u32,
    disp_h: u32,
    preload: usize,
    begin: usize,
    frame_cnt: usize,
    frame_dir: String,
    frame_fmt: String,
    thread_cnt: usize,
    thread_id: usize,
}

fn start_img_thread(
    opts: ImageThreadOptions,
    tx: channel::Sender<image::ImageBuffer<image::Rgb<u8>, Vec<u8>>>,
) -> Result<JoinHandle<()>, std::io::Error> {
    thread::Builder::new().name("bad_apple_imgs".to_string()).spawn(move || {
        let mut cur_frame = opts.begin;
        println!("[IMG Thread {}]: Started!", opts.thread_id);
        'outer: while cur_frame < opts.frame_cnt {
            let begin_frame = cur_frame;
            let (begin_ms, mut io_us, mut conv_us, mut decode_us) = (millis(), 0, 0, 0);
            while tx.len() < opts.preload {
                let mut last_us;
                if cur_frame >= opts.frame_cnt {
                    break;
                }
                if (cur_frame % opts.thread_cnt) != opts.thread_id {
                    // skip this frame as it'll be handled by another thread
                    cur_frame += 1;
                    continue;
                }

                let img_fname = format!("{}/{:03}.{}", opts.frame_dir, cur_frame + 1, opts.frame_fmt);

                // Open and read the image
                last_us = micros();
                let img_reader = match ImageReader::open(&img_fname) {
                    Ok(rdr) => rdr,
                    Err(err) => {
                        eprintln!(
                            "[IMG Thread {}]: Failed to load frame {} via {}",
                            opts.thread_id, cur_frame, img_fname
                        );
                        eprintln!("[IMG Thread {}]: Error: {:?}", opts.thread_id, err);
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
                            "[IMG Thread {}]: Failed to decode frame {} via {}", opts.thread_id,
                            cur_frame, img_fname
                        );
                        eprintln!("[IMG Thread {}]: Error: {:?}", opts.thread_id, err);
                        break 'outer;
                    }
                };
                decode_us += micros() - last_us;

                // Convert the image into a displayable format
                last_us = micros();
                let img_send = img_result
                    .resize_exact(opts.disp_w, opts.disp_h, FilterType::Triangle)
                    .to_rgb8();
                conv_us += micros() - last_us;

                tx.send(img_send).expect(&format!("[IMG Thread {}]: Failed to send image through channel?!", opts.thread_id).to_string());
                cur_frame += 1;
            }
            println!(
                "[IMG Thread {}]: Loaded frames {} to {}, took {}ms, io {}us, decode {}us, conversion {}us",
                opts.thread_id,
                begin_frame,
                cur_frame,
                millis() - begin_ms, io_us, decode_us, conv_us
            );
            if tx.len() >= opts.preload {
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
        }
        println!("[IMG Thread {}]: Stopped!", opts.thread_id);
    })
}

fn play_audio(frame_dir: String) -> Result<(Sink, OutputStream), String> {
    // Get the output stream
    let stream_handle = match rodio::OutputStreamBuilder::open_default_stream() {
        Ok(val) => val,
        Err(err) => {
            return Err(format!(
                "Failed to open default audio stream!\nError: {:?}",
                err
            ))
        }
    };
    // Load the sound file
    let file = match File::open(format!("{}/music.mp3", frame_dir)) {
        Ok(val) => val,
        Err(err) => return Err(format!("Failed to open audio file!\nError: {:?}", err)),
    };
    // Create a sink for the device
    let sink = rodio::Sink::connect_new(stream_handle.mixer());
    // Decode and play the sound file
    match Decoder::try_from(file) {
        Ok(source) => stream_handle.mixer().add(source),
        Err(err) => return Err(format!("Failed to play audio file!\nError: {:?}", err)),
    };
    Ok((sink, stream_handle))
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

    /// Frame formate to use
    #[arg(short, long, default_value = "jpg")]
    frame_format: String,

    // Image thread count, 0 will use the number of cores on the system
    #[arg(short, long, default_value_t = 0)]
    threads: usize,
}

fn main() {
    let args = Args::parse();
    println!("Using {} as frame directory!", args.directory);

    let mut fb = Framebuffer::new("/dev/fb0").unwrap();
    let gfx_mode = Framebuffer::set_kd_mode(KdMode::Graphics);
    if gfx_mode.is_err() {
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

    let img_thread_cnt = if args.threads == 0 {
        thread::available_parallelism().unwrap().get()
    } else {
        args.threads
    };

    let scale_w = gl.get_width() as u32;
    let scale_h = gl.get_height() as u32;

    // allocate vecs for each image thread and it's channel'
    let mut img_rx_channels: Vec<channel::Receiver<image::ImageBuffer<image::Rgb<u8>, Vec<u8>>>> =
        Vec::with_capacity(img_thread_cnt);
    let mut img_thread_handles: Vec<Result<JoinHandle<()>, std::io::Error>> =
        Vec::with_capacity(img_thread_cnt);

    // initialize an image threads for each channel
    for id in 0..img_thread_cnt {
        let (img_tx, img_rx) = channel::bounded::<RgbImage>(10);
        img_rx_channels.push(img_rx);
        img_thread_handles.push(start_img_thread(
            ImageThreadOptions {
                disp_w: scale_w,
                disp_h: scale_h,
                begin: 0,
                frame_cnt: total_frames,
                preload: preload_frames,
                frame_dir: args.directory.clone(),
                frame_fmt: args.frame_format.clone(),
                thread_cnt: img_thread_cnt,
                thread_id: id,
            },
            img_tx,
        ));
    }

    gl.clear(Color565::new(0, 0, 0));
    gl.push_buffer();
    std::thread::sleep(std::time::Duration::from_millis(args.init_delay));

    let audio_result = play_audio(args.directory);

    let frametime_ms = (1000 / args.framerate) as u128;
    let mut cur_frame = 0;
    println!("[GFX Thread]: Started!");
    let mut last_ms = 0;
    while cur_frame < total_frames {
        let cur_ms = millis();
        if cur_ms > last_ms + frametime_ms {
            last_ms = cur_ms;

            // a little over 60 fps
            match img_rx_channels[cur_frame % img_thread_cnt].try_recv() {
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
    for img_handle in img_thread_handles {
        img_handle
            .expect("the thread has been built")
            .join()
            .unwrap();
    }
    if let Ok((audio_sink, audio_stream)) = audio_result {
        audio_sink.sleep_until_end();
        drop(audio_stream);
    }

    if gfx_mode.is_ok() {
        let _ = Framebuffer::set_kd_mode(KdMode::Text);
    }
}
