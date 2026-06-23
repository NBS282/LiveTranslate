use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use serde::Serialize;
use serde_json;
use tauri::{AppHandle, Emitter};

use crate::translation::segmenter::{Segmenter, FRAME_SAMPLES_16K};

/// Minimum segment length in samples at 16 kHz (0.5 s).
/// Segments shorter than this are silently dropped.
const MIN_SEGMENT_SAMPLES: usize = 8_000;

/// Volume of the mic passthrough relative to full scale (0.0 – 1.0).
/// Low enough to be a "comfort signal" without drowning out the caller.
const PASSTHROUGH_GAIN: f32 = 0.20;

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

/// Drains any extra segments already queued behind `first`, returning the most
/// recent one and the number dropped. Translation on CPU is slower than real
/// time, so segments pile up during long speech; keeping only the latest bounds
/// the live latency instead of letting it grow without limit.
fn drain_to_latest(rx: &Receiver<Vec<i16>>, first: Vec<i16>) -> (Vec<i16>, usize) {
    let mut latest = first;
    let mut dropped = 0;
    while let Ok(newer) = rx.try_recv() {
        latest = newer;
        dropped += 1;
    }
    (latest, dropped)
}

/// Playback thread: plays each translated WAV to `output_device` serially, then
/// deletes the file and its temp dir. Kept separate from the worker so playback
/// (which runs at ~real time) never blocks translation of the next segment.
/// Sets `is_playing` while audio is rendering so the passthrough thread stays silent.
fn run_playback(
    rx: Receiver<PathBuf>,
    stop: Arc<AtomicBool>,
    output_device: String,
    is_playing: Arc<AtomicBool>,
) {
    while !stop.load(Ordering::Relaxed) {
        match rx.recv_timeout(std::time::Duration::from_millis(250)) {
            Ok(wav) => {
                is_playing.store(true, Ordering::Relaxed);
                if let Err(e) = play_wav_to_device(&wav, &output_device) {
                    eprintln!("playback error: {e}");
                }
                is_playing.store(false, Ordering::Relaxed);
                let _ = std::fs::remove_file(&wav);
                if let Some(dir) = wav.parent() {
                    let _ = std::fs::remove_dir(dir);
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
            Err(_) => break,
        }
    }
}

/// Passthrough thread: forwards raw 16 kHz mic frames to the output device at a reduced
/// gain so callers hear "someone is speaking" during the translation processing gap.
/// Silences itself (and flushes its buffer) while the playback thread is rendering a
/// translated WAV, so the two audio streams never overlap.
fn run_passthrough(
    rx: Receiver<Vec<i16>>,
    stop: Arc<AtomicBool>,
    is_playing: Arc<AtomicBool>,
    output_device_name: String,
) {
    use std::collections::VecDeque;

    let host = cpal::default_host();
    let device = if output_device_name.is_empty() {
        host.default_output_device()
    } else {
        host.output_devices()
            .ok()
            .and_then(|mut it| {
                it.find(|d| d.name().map(|n| n == output_device_name).unwrap_or(false))
            })
            .or_else(|| host.default_output_device())
    };

    let device = match device {
        Some(d) => d,
        None => {
            eprintln!("passthrough: no output device found");
            return;
        }
    };

    let native_cfg = match device.default_output_config() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("passthrough: config error: {e}");
            return;
        }
    };

    let native_rate = native_cfg.sample_rate().0 as usize;
    let native_ch = native_cfg.channels() as usize;
    // Cap at ~500 ms of buffered samples to prevent drift accumulation.
    let max_buf = native_rate * native_ch / 2;

    let buf: Arc<std::sync::Mutex<VecDeque<f32>>> =
        Arc::new(std::sync::Mutex::new(VecDeque::new()));
    let buf_cb = buf.clone();

    let stream_cfg = cpal::StreamConfig {
        channels: native_ch as u16,
        sample_rate: cpal::SampleRate(native_rate as u32),
        buffer_size: cpal::BufferSize::Default,
    };

    let stream = match device.build_output_stream(
        &stream_cfg,
        move |data: &mut [f32], _| {
            let mut b = buf_cb.lock().unwrap();
            for d in data.iter_mut() {
                *d = b.pop_front().unwrap_or(0.0);
            }
        },
        |e| eprintln!("passthrough stream error: {e}"),
        None,
    ) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("passthrough: stream build error: {e}");
            return;
        }
    };

    if let Err(e) = stream.play() {
        eprintln!("passthrough: stream play error: {e}");
        return;
    }

    let mut resampler = SimpleResampler::new(16_000, native_rate);

    while !stop.load(Ordering::Relaxed) {
        match rx.recv_timeout(std::time::Duration::from_millis(50)) {
            Ok(samples) => {
                if is_playing.load(Ordering::Relaxed) {
                    // Flush so stale mic audio doesn't bleed into the translated playback.
                    buf.lock().unwrap().clear();
                } else {
                    let f32_in: Vec<f32> = samples
                        .iter()
                        .map(|&s| s as f32 / i16::MAX as f32 * PASSTHROUGH_GAIN)
                        .collect();
                    let resampled = resampler.process(&f32_in);
                    let final_samples = convert_channels(&resampled, 1, native_ch);
                    let mut b = buf.lock().unwrap();
                    if b.len() + final_samples.len() <= max_buf {
                        b.extend(final_samples);
                    }
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
            Err(_) => break,
        }
    }
    // `stream` drops here, stopping cpal output automatically.
}

/// Worker thread: consumes segments from `rx`, translates each, emits a "phrase" event,
/// then hands the TTS audio to the playback thread via `play_tx`. Translation of the
/// next segment proceeds without waiting for playback to finish.
fn run_worker(
    rx: Receiver<Vec<i16>>,
    app: AppHandle,
    stop: Arc<AtomicBool>,
    play_tx: Sender<PathBuf>,
) {
    while !stop.load(Ordering::Relaxed) {
        match rx.recv_timeout(std::time::Duration::from_millis(250)) {
            Ok(samples) => {
                // Stay current: if requests fell behind and segments queued up,
                // keep only the most recent and drop the stale ones.
                let (samples, dropped) = drain_to_latest(&rx, samples);
                if dropped > 0 {
                    eprintln!("live: dropped {dropped} stale segment(s) to stay current");
                }

                // PTT diag: emit event so frontend can show producer→worker flow.
                let _ = app.emit("ptt-diag", serde_json::json!({
                    "event": "worker-received",
                    "samples": samples.len(),
                    "duration_s": samples.len() as f64 / 16_000.0,
                }));

                // Defense-in-depth: discard segments shorter than 0.5s (8000 samples at 16 kHz).
                // Whisper hallucinates or returns no text on very short clips; the VAD
                // min-voiced guard is the first line of defense, this is the fallback.
                if samples.len() < MIN_SEGMENT_SAMPLES {
                    let _ = app.emit("ptt-diag", serde_json::json!({
                        "event": "worker-discarded-too-short",
                        "samples": samples.len(),
                        "min": MIN_SEGMENT_SAMPLES,
                    }));
                    continue;
                }

                let translate_result = write_segment_wav(&samples).and_then(|p| {
                    let result = crate::translation::engine_server::translate(&p);
                    let _ = std::fs::remove_file(&p);
                    result
                });

                // 422 "transcription produced no text" means Whisper got audio but found
                // no speech — silence, breath, or mic bleed. Skip the event silently.
                if let Err(ref e) = translate_result {
                    let _ = app.emit("ptt-diag", serde_json::json!({
                        "event": "translate-error",
                        "error": e,
                    }));
                    if e.contains("transcription produced no text") {
                        continue;
                    }
                }

                // If the model returned the source unchanged the text was already
                // English (or too short/ambiguous to translate). Skip — it adds no
                // value to surface an identical ES/EN pair to the user.
                if let Ok(ref out) = translate_result {
                    if out.source_text.trim().to_lowercase()
                        == out.translated_text.trim().to_lowercase()
                    {
                        let _ = app.emit("ptt-diag", serde_json::json!({
                            "event": "skipped-source-equals-target",
                            "source": out.source_text,
                        }));
                        continue;
                    }
                }

                let evt = match &translate_result {
                    Ok(out) => PhraseEvent {
                        source_text: out.source_text.clone(),
                        translated_text: out.translated_text.clone(),
                        error: None,
                    },
                    Err(e) => PhraseEvent {
                        source_text: String::new(),
                        translated_text: String::new(),
                        error: Some(e.clone()),
                    },
                };
                let _ = app.emit("phrase", evt);

                if let Ok(out) = translate_result {
                    // Hand off to the playback thread so the next segment can be
                    // translated while this one plays. The playback thread owns
                    // cleanup of the WAV and its temp dir.
                    let _ = play_tx.send(out.output_wav);
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
            Err(_) => break,
        }
    }
}

/// Starts continuous capture on `device_name`, spawns producer + worker threads.
/// Returns a `LiveSession` whose `stop` flag can be set to end both threads.
pub fn start(
    device_name: &str,
    output_device_name: &str,
    app: AppHandle,
) -> Result<LiveSession, String> {
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
    let (play_tx, play_rx): (Sender<PathBuf>, Receiver<PathBuf>) = std::sync::mpsc::channel();
    let (pass_tx, pass_rx): (Sender<Vec<i16>>, Receiver<Vec<i16>>) = std::sync::mpsc::channel();
    let stop = Arc::new(AtomicBool::new(false));
    let is_playing = Arc::new(AtomicBool::new(false));

    // Passthrough thread: forwards mic audio at low gain during processing gaps.
    {
        let stop_clone = stop.clone();
        let is_playing_clone = is_playing.clone();
        let output_device = output_device_name.to_string();
        std::thread::spawn(move || {
            run_passthrough(pass_rx, stop_clone, is_playing_clone, output_device)
        });
    }

    // Playback thread: plays translated audio serially, decoupled from translation.
    {
        let stop_clone = stop.clone();
        let output_device = output_device_name.to_string();
        let is_playing_clone = is_playing.clone();
        std::thread::spawn(move || {
            run_playback(play_rx, stop_clone, output_device, is_playing_clone)
        });
    }

    // Worker thread: translate segments, emit events, enqueue audio for playback.
    {
        let app_clone = app.clone();
        let stop_clone = stop.clone();
        std::thread::spawn(move || run_worker(seg_rx, app_clone, stop_clone, play_tx));
    }

    // Producer thread: capture → resample → VAD → segment → send.
    let stop_prod = stop.clone();
    std::thread::spawn(move || {
        if let Err(e) = run_producer(device, in_rate, channels, seg_tx, pass_tx, stop_prod) {
            eprintln!("live producer error: {e}");
        }
    });

    Ok(LiveSession { stop })
}

/// Producer: opens a cpal input stream, downmixes to mono, resamples to 16 kHz,
/// feeds 30 ms frames into webrtc-vad, and pushes closed segments onto `seg_tx`.
/// Every frame is also forwarded to `pass_tx` for the comfort-passthrough thread.
fn run_producer(
    device: cpal::Device,
    in_rate: usize,
    channels: usize,
    seg_tx: Sender<Vec<i16>>,
    pass_tx: Sender<Vec<i16>>,
    stop: Arc<AtomicBool>,
) -> Result<(), String> {
    use webrtc_vad::{SampleRate, Vad, VadMode};

    // VAD cadence: ~240ms trailing silence closes a phrase (8 * 30ms),
    // ~240ms minimum voiced (8 * 30ms) to emit. Lower silence = more responsive,
    // but too low risks cutting mid-phrase. Tune here.
    // silence_close=8 (~240ms), min_voiced=8 (~240ms), max_frames=167 (~5s force cut)
    let mut segmenter = Segmenter::new(8, 8, 167);
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
                        let _ = pass_tx.send(frame_acc.clone());
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

fn convert_channels(samples: &[f32], from_ch: usize, to_ch: usize) -> Vec<f32> {
    match (from_ch, to_ch) {
        (1, 2) => samples.iter().flat_map(|&s| [s, s]).collect(),
        (2, 1) => samples
            .chunks(2)
            .map(|c| (c[0] + c.get(1).copied().unwrap_or(0.0)) / 2.0)
            .collect(),
        _ => samples.to_vec(),
    }
}

fn play_wav_to_device(wav_path: &std::path::Path, output_device_name: &str) -> Result<(), String> {
    // --- Read WAV ---
    let mut reader = hound::WavReader::open(wav_path).map_err(|e| e.to_string())?;
    let spec = reader.spec();
    let wav_rate = spec.sample_rate as usize;
    let wav_channels = spec.channels as usize;

    let samples_f32: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => reader.samples::<f32>().filter_map(|s| s.ok()).collect(),
        hound::SampleFormat::Int => {
            let max = (1i64 << (spec.bits_per_sample.saturating_sub(1))) as f32;
            reader
                .samples::<i32>()
                .filter_map(|s| s.ok())
                .map(|s| s as f32 / max)
                .collect()
        }
    };

    // --- Find device ---
    let host = cpal::default_host();
    let device = if output_device_name.is_empty() {
        host.default_output_device()
    } else {
        host.output_devices()
            .map_err(|e| e.to_string())?
            .find(|d| d.name().map(|n| n == output_device_name).unwrap_or(false))
            .or_else(|| host.default_output_device())
    }
    .ok_or_else(|| "no output device available".to_string())?;

    let native_cfg = device.default_output_config().map_err(|e| e.to_string())?;
    let native_rate = native_cfg.sample_rate().0 as usize;
    let native_channels = native_cfg.channels() as usize;

    // --- Resample ---
    let resampled = if native_rate != wav_rate {
        let mut r = SimpleResampler::new(wav_rate, native_rate);
        r.process(&samples_f32)
    } else {
        samples_f32
    };

    // --- Channel conversion ---
    let final_samples = convert_channels(&resampled, wav_channels, native_channels);
    let duration_secs = final_samples.len() as f32 / native_rate as f32 / native_channels as f32;

    // --- Build and play stream ---
    let samples = std::sync::Arc::new(std::sync::Mutex::new(final_samples.into_iter()));
    let samples_cb = std::sync::Arc::clone(&samples);

    let stream_cfg = cpal::StreamConfig {
        channels: native_channels as u16,
        sample_rate: cpal::SampleRate(native_rate as u32),
        buffer_size: cpal::BufferSize::Default,
    };

    let stream = device
        .build_output_stream(
            &stream_cfg,
            move |data: &mut [f32], _| {
                let mut iter = samples_cb.lock().unwrap();
                for d in data.iter_mut() {
                    *d = iter.next().unwrap_or(0.0);
                }
            },
            |e| eprintln!("playback stream error: {e}"),
            None,
        )
        .map_err(|e| e.to_string())?;

    stream.play().map_err(|e| e.to_string())?;
    std::thread::sleep(std::time::Duration::from_secs_f32(duration_secs + 0.1));

    Ok(())
}

/// Starts PTT capture: same thread layout as `start()` but uses `run_producer_ptt`
/// instead of the VAD-based producer. Recording is gated by `ptt_recording`.
pub fn start_ptt(
    device_name: &str,
    output_device_name: &str,
    app: AppHandle,
    ptt_recording: Arc<AtomicBool>,
) -> Result<LiveSession, String> {
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
    let (play_tx, play_rx): (Sender<PathBuf>, Receiver<PathBuf>) = std::sync::mpsc::channel();
    let (pass_tx, pass_rx): (Sender<Vec<i16>>, Receiver<Vec<i16>>) = std::sync::mpsc::channel();
    let stop = Arc::new(AtomicBool::new(false));
    let is_playing = Arc::new(AtomicBool::new(false));

    {
        let stop_clone = stop.clone();
        let is_playing_clone = is_playing.clone();
        let output_device = output_device_name.to_string();
        std::thread::spawn(move || {
            run_passthrough(pass_rx, stop_clone, is_playing_clone, output_device)
        });
    }
    {
        let stop_clone = stop.clone();
        let output_device = output_device_name.to_string();
        let is_playing_clone = is_playing.clone();
        std::thread::spawn(move || {
            run_playback(play_rx, stop_clone, output_device, is_playing_clone)
        });
    }
    {
        let app_clone = app.clone();
        let stop_clone = stop.clone();
        std::thread::spawn(move || run_worker(seg_rx, app_clone, stop_clone, play_tx));
    }

    let stop_prod = stop.clone();
    std::thread::spawn(move || {
        if let Err(e) = run_producer_ptt(
            device,
            in_rate,
            channels,
            seg_tx,
            pass_tx,
            stop_prod,
            ptt_recording,
        ) {
            eprintln!("ptt producer error: {e}");
        }
    });

    Ok(LiveSession { stop })
}

/// PTT producer: captures audio continuously but only accumulates samples while
/// `ptt_recording` is true. On the falling edge (true→false) it sends the entire
/// buffered recording to the worker as a single segment.
///
/// Implementation based on Handy's proven pattern: polls the recording flag at
/// the TOP of every loop iteration (every audio callback, ~10 ms) so the falling
/// edge is detected IMMEDIATELY — no race window between frame boundaries.
fn run_producer_ptt(
    device: cpal::Device,
    in_rate: usize,
    channels: usize,
    seg_tx: Sender<Vec<i16>>,
    pass_tx: Sender<Vec<i16>>,
    stop: Arc<AtomicBool>,
    ptt_recording: Arc<AtomicBool>,
) -> Result<(), String> {
    let (samp_tx, samp_rx) = std::sync::mpsc::channel::<Vec<f32>>();

    let config: cpal::StreamConfig = device
        .default_input_config()
        .map_err(|e| e.to_string())?
        .into();

    let stream = device
        .build_input_stream(
            &config,
            move |data: &[f32], _: &cpal::InputCallbackInfo| {
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

    let mut resampler = SimpleResampler::new(in_rate, 16_000);
    let mut frame_acc: Vec<i16> = Vec::with_capacity(FRAME_SAMPLES_16K);
    let mut ptt_buffer: Vec<i16> = Vec::new();
    let mut was_recording = false;

    while !stop.load(Ordering::Relaxed) {
        // Poll recording flag at the TOP of every loop iteration (every audio
        // callback, ~10 ms), NOT only at frame boundaries (~30 ms).
        // This mirrors Handy's proven approach — detect the falling edge
        // IMMEDIATELY with no race window between frame boundaries.
        let now_recording = ptt_recording.load(Ordering::Relaxed);

        // ── Rising edge: start fresh ───────────────────────────────────
        if now_recording && !was_recording {
            ptt_buffer.clear();
        }

        // ── Falling edge: release was just detected — flush NOW ────────
        if was_recording && !now_recording {
            if !ptt_buffer.is_empty() {
                if ptt_buffer.len() >= MIN_SEGMENT_SAMPLES {
                    let seg = std::mem::take(&mut ptt_buffer);
                    let _ = seg_tx.send(seg);
                } else {
                    ptt_buffer.clear();
                }
            }
        }

        was_recording = now_recording;

        // ── Audio processing (same as VAD mode) ────────────────────────
        match samp_rx.recv_timeout(std::time::Duration::from_millis(100)) {
            Ok(mono) => {
                for s in resampler.process(&mono) {
                    let sample = (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
                    frame_acc.push(sample);

                    if frame_acc.len() == FRAME_SAMPLES_16K {
                        if now_recording {
                            ptt_buffer.extend_from_slice(&frame_acc);
                            let _ = pass_tx.send(frame_acc.clone());
                        }
                        frame_acc.clear();
                    }
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
            Err(_) => break,
        }
    }

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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn mono_to_stereo_duplicates_each_sample() {
        let mono = vec![0.5f32, -0.3, 0.1];
        let stereo = convert_channels(&mono, 1, 2);
        assert_eq!(stereo, vec![0.5, 0.5, -0.3, -0.3, 0.1, 0.1]);
    }

    #[test]
    fn stereo_to_mono_averages_pairs() {
        let stereo = vec![0.6f32, -0.6, 0.2, 0.4];
        let mono = convert_channels(&stereo, 2, 1);
        assert!(
            (mono[0] - 0.0).abs() < 1e-5,
            "expected 0.0, got {}",
            mono[0]
        );
        assert!(
            (mono[1] - 0.3).abs() < 1e-5,
            "expected 0.3, got {}",
            mono[1]
        );
    }

    #[test]
    fn same_channel_count_is_passthrough() {
        let samples = vec![0.1f32, 0.2, 0.3];
        assert_eq!(convert_channels(&samples, 1, 1), samples);
    }

    #[test]
    fn play_nonexistent_wav_returns_err() {
        let result = play_wav_to_device(Path::new("does_not_exist_xyz.wav"), "");
        assert!(result.is_err(), "expected Err for missing WAV file");
    }

    #[test]
    fn drain_keeps_latest_and_counts_dropped() {
        let (tx, rx) = std::sync::mpsc::channel();
        tx.send(vec![2i16]).unwrap();
        tx.send(vec![3i16]).unwrap();
        let (latest, dropped) = drain_to_latest(&rx, vec![1i16]);
        assert_eq!(latest, vec![3i16]);
        assert_eq!(dropped, 2);
    }

    #[test]
    fn drain_no_backlog_returns_first_unchanged() {
        let (_tx, rx) = std::sync::mpsc::channel::<Vec<i16>>();
        let (latest, dropped) = drain_to_latest(&rx, vec![9i16]);
        assert_eq!(latest, vec![9i16]);
        assert_eq!(dropped, 0);
    }
}
