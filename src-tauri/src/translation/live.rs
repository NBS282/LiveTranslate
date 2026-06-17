use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use serde::Serialize;
use tauri::{AppHandle, Emitter};

use crate::translation::segmenter::{Segmenter, FRAME_SAMPLES_16K};

#[derive(Clone, Serialize)]
pub struct PhraseEvent {
    pub source_text: String,
    pub translated_text: String,
    pub error: Option<String>,
}

/// Handle to a running live session. Drop or call stop() to end it.
pub struct LiveSession {
    pub stop: Arc<AtomicBool>,
}

/// Writes 16 kHz mono i16 samples to a uniquely-named temp wav and returns its path.
fn write_segment_wav(samples: &[i16]) -> Result<PathBuf, String> {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let path = std::env::temp_dir().join(format!(
        "livetranslate-seg-{}-{}.wav",
        std::process::id(),
        unique
    ));
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 16_000,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w = hound::WavWriter::create(&path, spec).map_err(|e| e.to_string())?;
    for s in samples {
        w.write_sample(*s).map_err(|e| e.to_string())?;
    }
    w.finalize().map_err(|e| e.to_string())?;
    Ok(path)
}

/// Worker thread: consumes segments from `rx`, translates each, and emits a "phrase" event.
fn run_worker(rx: Receiver<Vec<i16>>, app: AppHandle, stop: Arc<AtomicBool>) {
    while !stop.load(Ordering::Relaxed) {
        match rx.recv_timeout(std::time::Duration::from_millis(250)) {
            Ok(samples) => {
                let evt = match write_segment_wav(&samples).and_then(|p| {
                    let result = crate::translation::engine_server::translate(&p);
                    let _ = std::fs::remove_file(&p);
                    result
                }) {
                    Ok(out) => PhraseEvent {
                        source_text: out.source_text,
                        translated_text: out.translated_text,
                        error: None,
                    },
                    Err(e) => PhraseEvent {
                        source_text: String::new(),
                        translated_text: String::new(),
                        error: Some(e),
                    },
                };
                let _ = app.emit("phrase", evt);
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
            Err(_) => break,
        }
    }
}

/// Starts continuous capture on `device_name`, spawns producer + worker threads.
/// Returns a `LiveSession` whose `stop` flag can be set to end both threads.
pub fn start(device_name: &str, app: AppHandle) -> Result<LiveSession, String> {
    let host = cpal::default_host();
    let device = host
        .input_devices()
        .map_err(|e| e.to_string())?
        .find(|d| d.name().map(|n| n == device_name).unwrap_or(false))
        .or_else(|| host.default_input_device())
        .ok_or_else(|| "no input device available".to_string())?;

    let default_config = device.default_input_config().map_err(|e| e.to_string())?;
    let in_rate = default_config.sample_rate().0 as usize;
    let channels = default_config.channels() as usize;

    let (seg_tx, seg_rx): (Sender<Vec<i16>>, Receiver<Vec<i16>>) = std::sync::mpsc::channel();
    let stop = Arc::new(AtomicBool::new(false));

    // Worker thread: translate segments and emit events.
    {
        let app_clone = app.clone();
        let stop_clone = stop.clone();
        std::thread::spawn(move || run_worker(seg_rx, app_clone, stop_clone));
    }

    // Producer thread: capture → resample → VAD → segment → send.
    let stop_prod = stop.clone();
    std::thread::spawn(move || {
        if let Err(e) = run_producer(device, in_rate, channels, seg_tx, stop_prod) {
            eprintln!("live producer error: {e}");
        }
    });

    Ok(LiveSession { stop })
}

/// Producer: opens a cpal input stream, downmixes to mono, resamples to 16 kHz,
/// feeds 30 ms frames into webrtc-vad, and pushes closed segments onto `seg_tx`.
fn run_producer(
    device: cpal::Device,
    in_rate: usize,
    channels: usize,
    seg_tx: Sender<Vec<i16>>,
    stop: Arc<AtomicBool>,
) -> Result<(), String> {
    use webrtc_vad::{SampleRate, Vad, VadMode};

    // VAD cadence: ~400ms trailing silence closes a phrase (13 * 30ms),
    // ~180ms minimum voiced (6 * 30ms) to emit. Lower silence = more responsive,
    // but too low risks cutting mid-phrase. Tune here.
    let mut segmenter = Segmenter::new(13, 6);
    let mut vad = Vad::new_with_rate_and_mode(SampleRate::Rate16kHz, VadMode::Quality);

    // Channel from the cpal callback (runs on an OS audio thread) to our loop below.
    let (samp_tx, samp_rx) = std::sync::mpsc::channel::<Vec<f32>>();

    let config: cpal::StreamConfig = device
        .default_input_config()
        .map_err(|e| e.to_string())?
        .into();

    let stream = device
        .build_input_stream(
            &config,
            move |data: &[f32], _: &cpal::InputCallbackInfo| {
                // Downmix multi-channel to mono by averaging.
                let ch = channels.max(1);
                let mut mono = Vec::with_capacity(data.len() / ch);
                for chunk in data.chunks(ch) {
                    let avg = chunk.iter().sum::<f32>() / chunk.len() as f32;
                    mono.push(avg);
                }
                let _ = samp_tx.send(mono);
            },
            |e| eprintln!("cpal input stream error: {e}"),
            None,
        )
        .map_err(|e| e.to_string())?;

    stream.play().map_err(|e| e.to_string())?;

    // Resample to 16 kHz and slice into 480-sample (30 ms) i16 frames.
    let mut resampler = SimpleResampler::new(in_rate, 16_000);
    let mut frame_acc: Vec<i16> = Vec::with_capacity(FRAME_SAMPLES_16K);

    while !stop.load(Ordering::Relaxed) {
        match samp_rx.recv_timeout(std::time::Duration::from_millis(100)) {
            Ok(mono) => {
                for s in resampler.process(&mono) {
                    let sample = (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
                    frame_acc.push(sample);
                    if frame_acc.len() == FRAME_SAMPLES_16K {
                        let voiced = vad.is_voice_segment(&frame_acc).unwrap_or(false);
                        if let Some(seg) = segmenter.push(&frame_acc, voiced) {
                            let _ = seg_tx.send(seg);
                        }
                        frame_acc.clear();
                    }
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
            Err(_) => break,
        }
    }

    // `stream` drops here, which stops the cpal capture automatically.
    Ok(())
}

/// Minimal linear interpolation resampler adequate for 16 kHz VAD/STT input.
/// rubato (declared in Cargo.toml) is available for a higher-quality replacement.
struct SimpleResampler {
    ratio: f32,
    pos: f32,
    last: f32,
}

impl SimpleResampler {
    fn new(from_rate: usize, to_rate: usize) -> Self {
        Self {
            ratio: from_rate as f32 / to_rate as f32,
            pos: 0.0,
            last: 0.0,
        }
    }

    fn process(&mut self, input: &[f32]) -> Vec<f32> {
        let mut out = Vec::new();
        for &x in input {
            while self.pos <= 1.0 {
                let interp = self.last + (x - self.last) * self.pos;
                out.push(interp);
                self.pos += self.ratio;
            }
            self.pos -= 1.0;
            self.last = x;
        }
        out
    }
}
