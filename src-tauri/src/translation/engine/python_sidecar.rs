use std::path::PathBuf;

use crate::translation::engine::{Decoded, DecodedWithAudio, TranslationEngine};
use crate::translation::engine_server::{self, LangPair};

/// Maximum failed-segment WAVs kept for diagnosis. These are raw mic audio, so
/// the directory is ring-buffered: oldest files are pruned when new ones arrive.
const MAX_FAILED_SEGMENTS: usize = 10;

/// Engine implementation that delegates decode + synthesis to the Python
/// sidecar over HTTP (see `engine_server`). This is the only engine variant
/// in Phase 1 — the trait exists purely as a seam for future native engines.
pub struct PythonSidecarEngine;

impl TranslationEngine for PythonSidecarEngine {
    fn decode_and_synthesize(
        &self,
        samples: &[i16],
        use_cloned_voice: bool,
        lang: &LangPair,
    ) -> Result<DecodedWithAudio, String> {
        let path = write_segment_wav(samples)?;
        let result = engine_server::translate_ex(&path, use_cloned_voice, lang);
        // Keep the audio of failed LONG segments for post-mortem: an "empty
        // transcription" on >=4s of VAD-voiced audio is a decode bug we can
        // only diagnose by re-running the exact input offline. Short failures
        // are breath tails (expected, benign) — retaining those would
        // accumulate raw mic audio on disk for no value.
        if result.is_err() && samples.len() >= 64_000 {
            keep_failed_segment(&path);
        } else {
            let _ = std::fs::remove_file(&path);
        }
        result.map(|out| DecodedWithAudio {
            decoded: Decoded {
                source_text: out.source_text,
                translated_text: out.translated_text,
            },
            output_wav: out.output_wav,
        })
    }

    fn decode_partial(&self, samples: &[i16], lang: &LangPair) -> Result<String, String> {
        let path = write_segment_wav(samples)?;
        let result = engine_server::transcribe_partial(&path, lang);
        let _ = std::fs::remove_file(&path);
        result
    }
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

/// Moves a failed segment WAV into <root>/logs for offline post-mortem,
/// pruning the oldest kept files beyond `MAX_FAILED_SEGMENTS`.
fn keep_failed_segment(p: &std::path::Path) {
    let logs = crate::translation::sidecar::repo_root().join("logs");
    let keep = logs.join(format!(
        "failed-{}",
        p.file_name().unwrap_or_default().to_string_lossy()
    ));
    let _ = std::fs::create_dir_all(&logs);
    // %TEMP% and the data dir can live on different volumes in a packaged
    // install, where rename fails cross-device — fall back to copy+delete.
    let kept = std::fs::rename(p, &keep).is_ok()
        || (std::fs::copy(p, &keep).is_ok() && {
            let _ = std::fs::remove_file(p);
            true
        });
    if !kept {
        let _ = std::fs::remove_file(p);
        return;
    }
    eprintln!("live: failed segment kept at {}", keep.display());

    // Prune: keep only the newest MAX_FAILED_SEGMENTS failed-*.wav files.
    if let Ok(entries) = std::fs::read_dir(&logs) {
        let mut failed: Vec<_> = entries
            .filter_map(Result::ok)
            .filter(|e| {
                e.file_name().to_string_lossy().starts_with("failed-")
                    && e.file_name().to_string_lossy().ends_with(".wav")
            })
            .collect();
        // Names embed a nanosecond timestamp, so lexical order is age order.
        failed.sort_by_key(|e| e.file_name());
        while failed.len() > MAX_FAILED_SEGMENTS {
            let oldest = failed.remove(0);
            let _ = std::fs::remove_file(oldest.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_segment_wav_roundtrips_samples() {
        let samples: Vec<i16> = vec![0, 100, -100, i16::MAX, i16::MIN];
        let path = write_segment_wav(&samples).unwrap();
        assert!(path.exists());

        let mut reader = hound::WavReader::open(&path).unwrap();
        let spec = reader.spec();
        assert_eq!(spec.sample_rate, 16_000);
        assert_eq!(spec.channels, 1);
        let read_back: Vec<i16> = reader.samples::<i16>().map(|s| s.unwrap()).collect();
        assert_eq!(read_back, samples);

        let _ = std::fs::remove_file(&path);
    }
}
