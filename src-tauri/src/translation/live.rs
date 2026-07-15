use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use serde::Serialize;
use serde_json;
use tauri::{AppHandle, Emitter};

use crate::translation::engine::TranslationEngine;
use crate::translation::segmenter::{Segmenter, FRAME_SAMPLES_16K};

/// Minimum segment length in samples at 16 kHz (0.5 s).
/// Segments shorter than this are silently dropped.
const MIN_SEGMENT_SAMPLES: usize = 8_000;

/// Volume of the mic passthrough relative to full scale (0.0 – 1.0).
/// Low enough to be a "comfort signal" without drowning out the caller.
const PASSTHROUGH_GAIN: f32 = 0.20;

/// True when `text` ends a sentence: trims trailing whitespace and closing
/// quotes, then checks whether the last remaining char is `.`, `!`, `?`, or an
/// ellipsis (`…`). Used to arm the segmenter's fast-close window so the next
/// brief VAD dip cuts at a real inter-sentence pause.
fn ends_sentence(text: &str) -> bool {
    let trimmed = text
        .trim_end()
        .trim_end_matches(['"', '\'', '\u{201d}', '\u{2019}'])
        .trim_end();
    matches!(trimmed.chars().last(), Some('.' | '!' | '?' | '…'))
}

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

/// Bundles the per-session translation parameters passed into `run_worker`.
/// Grouped together (rather than three separate positional args) so adding a
/// config knob — like the engine seam below — doesn't grow `run_worker`'s
/// argument list further.
#[derive(Clone)]
struct WorkerConfig {
    use_cloned_voice: bool,
    lang: crate::translation::engine_server::LangPair,
    engine: Arc<dyn TranslationEngine>,
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
                eprintln!("live: playback start {}", wav.display());
                is_playing.store(true, Ordering::Relaxed);
                if let Err(e) = play_wav_to_device(&wav, &output_device) {
                    eprintln!("playback error: {e}");
                }
                is_playing.store(false, Ordering::Relaxed);
                eprintln!("live: playback done  {}", wav.display());
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
    config: WorkerConfig,
    pending_finals: Arc<AtomicUsize>,
) {
    // Tracks the previous segment's translated text (trimmed, lowercased). Canary
    // AST never produces a source_text, so the source==target echo guard below
    // is dead for it; this catches the same echo/feedback risk (output device
    // audio bleeding back into the mic) by comparing consecutive translations
    // instead. Refreshed on every successful translation, independent of the
    // guards below, so a run of repeats is judged against the immediately
    // preceding segment.
    let mut prev_translated_lower = String::new();

    while !stop.load(Ordering::Relaxed) {
        match rx.recv_timeout(std::time::Duration::from_millis(250)) {
            Ok(samples) => {
                // Consecutive speech is content, not staleness: every received
                // segment is translated in order so a long utterance isn't
                // reduced to only its last chunk. `pending_finals` tracks how
                // many segments are queued/in-flight; the partial-decode thread
                // yields while it's nonzero so finals never wait behind partials.
                let backlog = pending_finals.load(Ordering::Relaxed);
                if backlog > 2 {
                    eprintln!("live: translation backlog {backlog} segments");
                }

                eprintln!(
                    "live: segment received ({:.1}s, backlog {backlog})",
                    samples.len() as f64 / 16_000.0
                );

                // PTT diag: emit event so frontend can show producer→worker flow.
                let _ = app.emit(
                    "ptt-diag",
                    serde_json::json!({
                        "event": "worker-received",
                        "samples": samples.len(),
                        "duration_s": samples.len() as f64 / 16_000.0,
                    }),
                );

                // Defense-in-depth: discard segments shorter than 0.5s (8000 samples at 16 kHz).
                // Whisper hallucinates or returns no text on very short clips; the VAD
                // min-voiced guard is the first line of defense, this is the fallback.
                if samples.len() < MIN_SEGMENT_SAMPLES {
                    eprintln!("live: segment skipped (too short)");
                    let _ = app.emit(
                        "ptt-diag",
                        serde_json::json!({
                            "event": "worker-discarded-too-short",
                            "samples": samples.len(),
                            "min": MIN_SEGMENT_SAMPLES,
                        }),
                    );
                    pending_finals.fetch_sub(1, Ordering::Relaxed);
                    continue;
                }

                let translate_result = config.engine.decode_and_synthesize(
                    &samples,
                    config.use_cloned_voice,
                    &config.lang,
                );

                match &translate_result {
                    Ok(out) => eprintln!(
                        "live: segment translated -> {:.60}",
                        out.decoded.translated_text
                    ),
                    Err(e) => eprintln!("live: segment translate FAILED: {e:.120}"),
                }

                // 422 "transcription produced no text" means Whisper got audio but found
                // no speech — silence, breath, or mic bleed. Skip the event silently.
                if let Err(ref e) = translate_result {
                    let _ = app.emit(
                        "ptt-diag",
                        serde_json::json!({
                            "event": "translate-error",
                            "error": e,
                        }),
                    );
                    if e.contains("transcription produced no text") {
                        pending_finals.fetch_sub(1, Ordering::Relaxed);
                        continue;
                    }
                }

                // If the model returned the source unchanged the text was already
                // English (or too short/ambiguous to translate). Skip — it adds no
                // value to surface an identical ES/EN pair to the user.
                if let Ok(ref out) = translate_result {
                    if out.decoded.source_text.trim().to_lowercase()
                        == out.decoded.translated_text.trim().to_lowercase()
                    {
                        eprintln!("live: segment skipped (source == target)");
                        let _ = app.emit(
                            "ptt-diag",
                            serde_json::json!({
                                "event": "skipped-source-equals-target",
                                "source": out.decoded.source_text,
                            }),
                        );
                        pending_finals.fetch_sub(1, Ordering::Relaxed);
                        continue;
                    }
                }

                // Cheap echo mitigation for Canary AST: source_text is always empty
                // for AST, so the source==target guard above can never fire for it.
                // If the translated text repeats the previous segment's output
                // verbatim, it's very likely echo/feedback (output device audio
                // bleeding into the mic) rather than genuinely repeated speech.
                if let Ok(ref out) = translate_result {
                    let translated_lower = out.decoded.translated_text.trim().to_lowercase();
                    let is_echo = out.decoded.source_text.is_empty()
                        && !translated_lower.is_empty()
                        && translated_lower == prev_translated_lower;
                    prev_translated_lower = translated_lower;
                    if is_echo {
                        eprintln!("live: segment skipped (duplicate translation)");
                        let _ = app.emit(
                            "ptt-diag",
                            serde_json::json!({
                                "event": "skipped-duplicate-translation",
                                "text": out.decoded.translated_text,
                            }),
                        );
                        pending_finals.fetch_sub(1, Ordering::Relaxed);
                        continue;
                    }
                }

                let evt = match &translate_result {
                    Ok(out) => PhraseEvent {
                        source_text: out.decoded.source_text.clone(),
                        translated_text: out.decoded.translated_text.clone(),
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
                    eprintln!("live: segment enqueued for playback");
                    let _ = play_tx.send(out.output_wav);
                }

                pending_finals.fetch_sub(1, Ordering::Relaxed);
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
    use_cloned_voice: bool,
    lang: crate::translation::engine_server::LangPair,
    engine: Arc<dyn TranslationEngine>,
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
    // Shared by producer, worker, and the partial-decode thread below: the
    // producer increments right after handing a segment to the worker, the
    // worker decrements when it finishes that segment (every exit path —
    // success, error, or discard), and the partial thread yields its tick
    // while this is nonzero so finals never wait behind partials for the
    // decode lock.
    let pending_finals = Arc::new(AtomicUsize::new(0));

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
        let pending_worker = pending_finals.clone();
        let worker_config = WorkerConfig {
            use_cloned_voice,
            lang: lang.clone(),
            engine: engine.clone(),
        };
        std::thread::spawn(move || {
            run_worker(
                seg_rx,
                app_clone,
                stop_clone,
                play_tx,
                worker_config,
                pending_worker,
            )
        });
    }

    // Shared with the partial-decode thread below: the producer pushes frames in,
    // the partial thread snapshots the still-open segment out. Continuous mode only —
    // PTT (`start_ptt`) does not use a `Segmenter` at all, so it never shares this lock.
    let segmenter = Arc::new(Mutex::new(Segmenter::new(8, 8, 267)));

    // Partial subtitles: translate the open segment every ~1.2s so the user
    // sees text while still speaking. One request in flight; a tick is
    // skipped when the previous decode is still running or nothing new
    // was captured.
    {
        let seg_for_partials = segmenter.clone();
        let app_partials = app.clone();
        let stop_partials = stop.clone();
        let pending_partials = pending_finals.clone();
        let lang_partials = lang.clone();
        let engine_partials = engine.clone();
        std::thread::spawn(move || {
            let mut last_len = 0usize;
            while !stop_partials.load(Ordering::Relaxed) {
                std::thread::sleep(std::time::Duration::from_millis(1200));
                // Finals take priority: skip this tick entirely rather than
                // contend with the worker for the Python-side decode lock
                // while segments are queued or in flight.
                if pending_partials.load(Ordering::Relaxed) > 0 {
                    continue;
                }
                // A poisoned lock means the other thread panicked mid-push/snapshot;
                // the segmenter state is still sound (Vec operations don't tear), so
                // recover the guard instead of killing the live session.
                // Snapshot and generation are read under ONE guard so they describe
                // the same segment: the generation lets us detect, after the slow
                // decode below, whether that segment already closed in the meantime.
                let (snap, gen_at_snapshot) = {
                    let seg = seg_for_partials.lock().unwrap_or_else(|e| e.into_inner());
                    (seg.snapshot(), seg.generation())
                };
                let Some(samples) = snap else {
                    if last_len != 0 {
                        last_len = 0;
                        // Re-check right before emitting: stop() may have landed
                        // while we were locking/snapshotting, and a partial must
                        // never reach the UI after the session is torn down.
                        if !stop_partials.load(Ordering::Relaxed) {
                            let _ = app_partials.emit("partial", serde_json::json!({ "text": "" }));
                        }
                    }
                    continue;
                };
                // Under 1s of audio the decode returns noise; over-eager ticks
                // also starve the final /translate call on weak CPUs.
                if samples.len() < 16_000 || samples.len() == last_len {
                    continue;
                }
                last_len = samples.len();
                // decode_partial owns the temp-WAV write + cleanup internally
                // (see engine::python_sidecar) — this call site only deals in
                // samples in, text out, same as decode_and_synthesize above.
                let decode = engine_partials.decode_partial(&samples, &lang_partials);

                // Interpreter-style fast close: arm only when the in-progress
                // translation reads as a complete sentence AND we captured
                // enough audio (2s) to trust the punctuation isn't a
                // mid-utterance artifact. A failed or empty decode disarms —
                // a stale "sentence ended" must not survive up to 10s of
                // decode failures while the user is mid-sentence.
                let arm = samples.len() >= 32_000 && decode.as_deref().is_ok_and(ends_sentence);
                {
                    // The decode can take seconds; the segment we analyzed may
                    // have closed meanwhile (normal 240ms silence), resetting
                    // the flag and starting a new segment. Only touch the flag
                    // if the generation is unchanged — otherwise our decision
                    // is stale and would mis-arm a brand-new segment.
                    let mut seg = seg_for_partials.lock().unwrap_or_else(|e| e.into_inner());
                    if seg.generation() == gen_at_snapshot {
                        if arm {
                            eprintln!("live: fast-close armed (sentence end seen)");
                        }
                        seg.set_fast_close(arm);
                    }
                }

                if let Ok(text) = decode {
                    if !text.is_empty() {
                        // Re-check right before emitting: the decode above can
                        // take up to the 10s timeout, long enough for stop() to
                        // have landed in the meantime.
                        if !stop_partials.load(Ordering::Relaxed) {
                            let _ =
                                app_partials.emit("partial", serde_json::json!({ "text": text }));
                        }
                    }
                }
            }
        });
    }

    // Producer thread: capture → resample → VAD → segment → send.
    let stop_prod = stop.clone();
    std::thread::spawn(move || {
        if let Err(e) = run_producer(
            device,
            in_rate,
            channels,
            seg_tx,
            pass_tx,
            stop_prod,
            segmenter,
            pending_finals,
        ) {
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
    segmenter: Arc<Mutex<Segmenter>>,
    pending_finals: Arc<AtomicUsize>,
) -> Result<(), String> {
    use webrtc_vad::{SampleRate, Vad, VadMode};

    // Segmenter cadence (silence_close=8 ~240ms, min_voiced=8 ~240ms, max_frames=267
    // ~8s force cut) is fixed at construction in `start()`, shared via `Arc<Mutex<_>>`
    // with the partial-decode thread so it can snapshot the still-open segment.
    // 8s (was 5s) so a normal long sentence isn't force-cut mid-phrase before the
    // speaker's natural pause closes it — the 5s cap clipped the end of >5s phrases.
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
                        // Lock only for the push call itself — never held across
                        // `seg_tx.send` or any I/O — so the partial-decode thread's
                        // snapshot() never blocks on this producer for long.
                        let closed = segmenter
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .push(&frame_acc, voiced);
                        if let Some(seg) = closed {
                            eprintln!("live: segment closed ({:.1}s)", seg.len() as f64 / 16_000.0);
                            if seg_tx.send(seg).is_ok() {
                                pending_finals.fetch_add(1, Ordering::Relaxed);
                            }
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
    let mut final_samples = convert_channels(&resampled, wav_channels, native_channels);

    // Append a silent tail so the LAST thing fed to the device is silence,
    // not speech. The drain flag below flips when the callback consumes the
    // final sample of THIS vector — any output latency past our drain pad
    // (device buffers, VB-Cable's internal ring) then swallows silence
    // instead of clipping the end of the phrase.
    let tail = tail_silence_samples(native_rate, native_channels, PLAYBACK_TAIL_SILENCE_MS);
    final_samples.extend(std::iter::repeat(0.0f32).take(tail));

    let duration_secs = final_samples.len() as f32 / native_rate as f32 / native_channels as f32;

    // --- Build and play stream ---
    let samples = std::sync::Arc::new(std::sync::Mutex::new(final_samples.into_iter()));
    let samples_cb = std::sync::Arc::clone(&samples);

    let stream_cfg = cpal::StreamConfig {
        channels: native_channels as u16,
        sample_rate: cpal::SampleRate(native_rate as u32),
        buffer_size: cpal::BufferSize::Default,
    };

    // Dropping the stream stops output instantly, so we must not leave this
    // function until the device has actually rendered every sample. A fixed
    // sleep measured from play() undershoots: WASAPI stream startup (device
    // init + buffer priming) delays the first audible sample by 50-300 ms,
    // which used to cut the tail of every phrase — badly so on short clips.
    // Instead the callback flags when it has consumed the final sample, and
    // we wait for that condition plus a drain pad for the last buffer.
    let drained = Arc::new(AtomicBool::new(false));
    let drained_cb = drained.clone();

    let stream = device
        .build_output_stream(
            &stream_cfg,
            move |data: &mut [f32], _| {
                let mut iter = samples_cb.lock().unwrap();
                let mut exhausted = false;
                for d in data.iter_mut() {
                    match iter.next() {
                        Some(s) => *d = s,
                        None => {
                            *d = 0.0;
                            exhausted = true;
                        }
                    }
                }
                if exhausted {
                    drained_cb.store(true, Ordering::Relaxed);
                }
            },
            |e| eprintln!("playback stream error: {e}"),
            None,
        )
        .map_err(|e| e.to_string())?;

    stream.play().map_err(|e| e.to_string())?;

    // Generous timeout: covers duration + any plausible startup latency, and
    // guarantees we never hang if the device dies mid-playback.
    let started = std::time::Instant::now();
    let timeout = std::time::Duration::from_secs_f32(duration_secs + 2.0);
    if !wait_until_true(&drained, timeout) {
        eprintln!("playback: drain flag never set, timed out after {timeout:?}");
    }
    // Diagnostic: how far the callback-side drain ran ahead of real time. A
    // large gap means the device buffers deep and the tail relies on the
    // silence pad above — data we need if clipping is ever reported again.
    eprintln!(
        "live: playback drained in {:.2}s (audio+tail {:.2}s)",
        started.elapsed().as_secs_f32(),
        duration_secs,
    );
    // The flag flips when the callback FILLS the last buffer; the device still
    // has up to that buffer plus its output latency left to render.
    std::thread::sleep(std::time::Duration::from_millis(PLAYBACK_DRAIN_PAD_MS));

    Ok(())
}

/// Milliseconds to keep the output stream alive after the callback consumed
/// the last sample: one hardware buffer + typical WASAPI output latency.
const PLAYBACK_DRAIN_PAD_MS: u64 = 250;

/// Milliseconds of silence appended after the phrase so residual device
/// latency (WASAPI buffers, VB-Cable's ring) clips silence, never speech.
const PLAYBACK_TAIL_SILENCE_MS: usize = 300;

/// Interleaved sample count for `ms` of silence at the given rate/channels.
fn tail_silence_samples(rate: usize, channels: usize, ms: usize) -> usize {
    rate * channels * ms / 1000
}

/// Polls `flag` every 10 ms until it is true (returns true) or `timeout`
/// elapses (returns false).
fn wait_until_true(flag: &AtomicBool, timeout: std::time::Duration) -> bool {
    let start = std::time::Instant::now();
    while start.elapsed() < timeout {
        if flag.load(Ordering::Relaxed) {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    flag.load(Ordering::Relaxed)
}

/// Starts PTT capture: same thread layout as `start()` but uses `run_producer_ptt`
/// instead of the VAD-based producer. Recording is gated by `ptt_recording`.
pub fn start_ptt(
    device_name: &str,
    output_device_name: &str,
    app: AppHandle,
    ptt_recording: Arc<AtomicBool>,
    use_cloned_voice: bool,
    lang: crate::translation::engine_server::LangPair,
    engine: Arc<dyn TranslationEngine>,
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
    // PTT has no partial-decode thread, but `run_worker` is shared with the
    // continuous path and always expects this counter.
    let pending_finals = Arc::new(AtomicUsize::new(0));

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
        let pending_worker = pending_finals.clone();
        let worker_config = WorkerConfig {
            use_cloned_voice,
            lang: lang.clone(),
            engine: engine.clone(),
        };
        std::thread::spawn(move || {
            run_worker(
                seg_rx,
                app_clone,
                stop_clone,
                play_tx,
                worker_config,
                pending_worker,
            )
        });
    }

    let stop_prod = stop.clone();
    let app_prod = app.clone();
    std::thread::spawn(move || {
        if let Err(e) = run_producer_ptt(
            device,
            in_rate,
            channels,
            seg_tx,
            pass_tx,
            stop_prod,
            ptt_recording,
            pending_finals,
            app_prod,
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
    pending_finals: Arc<AtomicUsize>,
    app: AppHandle,
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
    // Throttles the mic-level events emitted for the overlay's recording bars.
    let mut level_frame: u64 = 0;

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
                    if seg_tx.send(seg).is_ok() {
                        pending_finals.fetch_add(1, Ordering::Relaxed);
                    }
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
                            // Emit a throttled RMS level (0..1 over the 30 ms
                            // frame) so the overlay's recording bars track the
                            // speaker's actual voice, ~every 60 ms.
                            level_frame = level_frame.wrapping_add(1);
                            if level_frame % 2 == 0 {
                                let sum_sq: f64 = frame_acc
                                    .iter()
                                    .map(|&s| {
                                        let f = s as f64 / i16::MAX as f64;
                                        f * f
                                    })
                                    .sum();
                                let rms = (sum_sq / frame_acc.len() as f64).sqrt() as f32;
                                let _ = app.emit("mic-level", serde_json::json!({ "level": rms }));
                            }
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
    fn ends_sentence_detects_terminal_punctuation() {
        assert!(ends_sentence("Hello there."));
        assert!(ends_sentence("Wait!"));
        assert!(ends_sentence("Really?"));
        assert!(ends_sentence("Well…"));
    }

    #[test]
    fn ends_sentence_handles_trailing_closing_quotes() {
        assert!(ends_sentence("She said \"stop.\""));
        assert!(ends_sentence("She said \"stop.\" "));
        assert!(ends_sentence("He asked 'why?'"));
    }

    #[test]
    fn ends_sentence_false_for_mid_sentence_or_empty() {
        assert!(!ends_sentence(""));
        assert!(!ends_sentence("   "));
        assert!(!ends_sentence("and then he"));
        assert!(!ends_sentence("wait, so"));
    }

    #[test]
    fn mono_to_stereo_duplicates_each_sample() {
        let mono = vec![0.5f32, -0.3, 0.1];
        let stereo = convert_channels(&mono, 1, 2);
        assert_eq!(stereo, vec![0.5, 0.5, -0.3, -0.3, 0.1, 0.1]);
    }

    #[test]
    fn tail_silence_matches_rate_and_channels() {
        // 300 ms at 48 kHz stereo = 48000 * 2 * 0.3 interleaved samples.
        assert_eq!(tail_silence_samples(48_000, 2, 300), 28_800);
        assert_eq!(tail_silence_samples(16_000, 1, 300), 4_800);
        assert_eq!(tail_silence_samples(44_100, 2, 0), 0);
    }

    #[test]
    fn wait_until_returns_true_when_flag_set() {
        let flag = Arc::new(AtomicBool::new(false));
        let flag_setter = flag.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(30));
            flag_setter.store(true, Ordering::Relaxed);
        });

        let ok = wait_until_true(&flag, std::time::Duration::from_secs(2));

        assert!(ok);
    }

    #[test]
    fn wait_until_times_out_when_flag_never_set() {
        let flag = Arc::new(AtomicBool::new(false));

        let start = std::time::Instant::now();
        let ok = wait_until_true(&flag, std::time::Duration::from_millis(50));

        assert!(!ok);
        assert!(start.elapsed() >= std::time::Duration::from_millis(50));
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
}
