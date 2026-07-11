//! Phase 0 spike: validate transcribe.cpp GGUF inference for LiveTranslate.
//!
//! Usage:
//!   gguf-stt <model.gguf> <input.wav> [--task asr|ast] [--src es] [--tgt en] [--threads N]
//!
//! Prints decoded text, model load time, decode wall-clock, RTF, and process
//! RSS so results are comparable against the current Python pipeline baseline.

use std::process::ExitCode;
use std::time::Instant;

use transcribe_cpp::{Model, RunOptions, Task};

const TARGET_SAMPLE_RATE: u32 = 16_000;

struct Args {
    model_path: String,
    wav_path: String,
    task: Task,
    src: Option<String>,
    tgt: Option<String>,
    threads: i32,
}

fn parse_args() -> Result<Args, String> {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    if argv.len() < 2 {
        return Err(
            "usage: gguf-stt <model.gguf> <input.wav> [--task asr|ast] [--src es] [--tgt en] [--threads N]"
                .into(),
        );
    }
    let mut args = Args {
        model_path: argv[0].clone(),
        wav_path: argv[1].clone(),
        task: Task::Transcribe,
        src: None,
        tgt: None,
        threads: 0,
    };
    let mut i = 2;
    while i < argv.len() {
        let flag = argv[i].as_str();
        let value = argv
            .get(i + 1)
            .ok_or_else(|| format!("missing value for {flag}"))?;
        match flag {
            "--task" => {
                args.task = match value.as_str() {
                    "asr" => Task::Transcribe,
                    "ast" => Task::Translate,
                    other => return Err(format!("unknown task: {other} (use asr|ast)")),
                };
            }
            "--src" => args.src = Some(value.clone()),
            "--tgt" => args.tgt = Some(value.clone()),
            "--threads" => {
                args.threads = value
                    .parse()
                    .map_err(|_| format!("invalid --threads: {value}"))?;
            }
            other => return Err(format!("unknown flag: {other}")),
        }
        i += 2;
    }
    Ok(args)
}

/// Read any PCM WAV (i16/f32, any rate, any channel count) and convert to
/// 16 kHz mono f32 in [-1, 1] via downmix + linear resampling.
fn load_wav_as_16k_mono(path: &str) -> Result<(Vec<f32>, f64), String> {
    let mut reader = hound::WavReader::open(path).map_err(|e| format!("open {path}: {e}"))?;
    let spec = reader.spec();
    let channels = spec.channels as usize;

    let interleaved: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Int => {
            let max = (1i64 << (spec.bits_per_sample - 1)) as f32;
            reader
                .samples::<i32>()
                .map(|s| s.map(|v| v as f32 / max))
                .collect::<Result<_, _>>()
                .map_err(|e| format!("read samples: {e}"))?
        }
        hound::SampleFormat::Float => reader
            .samples::<f32>()
            .collect::<Result<_, _>>()
            .map_err(|e| format!("read samples: {e}"))?,
    };

    let mono: Vec<f32> = interleaved
        .chunks_exact(channels)
        .map(|frame| frame.iter().sum::<f32>() / channels as f32)
        .collect();
    let duration_s = mono.len() as f64 / spec.sample_rate as f64;

    if spec.sample_rate == TARGET_SAMPLE_RATE {
        return Ok((mono, duration_s));
    }

    let ratio = spec.sample_rate as f64 / TARGET_SAMPLE_RATE as f64;
    let out_len = (mono.len() as f64 / ratio).floor() as usize;
    let mut out = Vec::with_capacity(out_len);
    for i in 0..out_len {
        let pos = i as f64 * ratio;
        let idx = pos as usize;
        let frac = (pos - idx as f64) as f32;
        let a = mono[idx];
        let b = *mono.get(idx + 1).unwrap_or(&a);
        out.push(a + (b - a) * frac);
    }
    Ok((out, duration_s))
}

fn rss_mb() -> f64 {
    memory_stats::memory_stats()
        .map(|s| s.physical_mem as f64 / (1024.0 * 1024.0))
        .unwrap_or(0.0)
}

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };

    let (pcm, duration_s) = match load_wav_as_16k_mono(&args.wav_path) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("wav error: {e}");
            return ExitCode::FAILURE;
        }
    };
    println!(
        "audio: {} ({duration_s:.1}s, {} samples @16k mono)",
        args.wav_path,
        pcm.len()
    );
    println!("rss before load: {:.0} MB", rss_mb());

    let t0 = Instant::now();
    let model = match Model::load(&args.model_path) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("model load error: {e}");
            return ExitCode::FAILURE;
        }
    };
    let load_s = t0.elapsed().as_secs_f64();
    let caps = model.capabilities();
    println!(
        "model: arch={} variant={} backend={} | load {load_s:.2}s | rss {:.0} MB",
        model.arch(),
        model.variant(),
        model.backend(),
        rss_mb()
    );
    println!(
        "caps: translate={} targets={:?} languages={:?} max_audio_ms={}",
        caps.supports_translate, caps.translate_target_languages, caps.languages, caps.max_audio_ms
    );

    let mut session = match model.session_with(&transcribe_cpp::SessionOptions {
        n_threads: args.threads,
        ..Default::default()
    }) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("session error: {e}");
            return ExitCode::FAILURE;
        }
    };

    let options = RunOptions {
        task: args.task,
        language: args.src.clone(),
        target_language: args.tgt.clone(),
        ..Default::default()
    };

    let t1 = Instant::now();
    let transcript = match session.run(&pcm, &options) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("run error: {e}");
            return ExitCode::FAILURE;
        }
    };
    let run_s = t1.elapsed().as_secs_f64();

    println!("---");
    println!("text: {}", transcript.text);
    println!("---");
    println!(
        "decode: {run_s:.2}s | RTF {:.3} | timings: mel {:.0}ms encode {:.0}ms decode {:.0}ms",
        run_s / duration_s,
        transcript.timings.mel_ms,
        transcript.timings.encode_ms,
        transcript.timings.decode_ms
    );
    println!("rss after run: {:.0} MB", rss_mb());
    ExitCode::SUCCESS
}
