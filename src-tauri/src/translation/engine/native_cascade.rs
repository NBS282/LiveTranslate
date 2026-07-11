//! Native (in-process) STT cascade engine: `transcribe_cpp` GGUF ASR runs
//! locally, then MarianMT translation and Piper/Pocket-TTS synthesis are
//! delegated to the Python sidecar's `/mt` and `/tts` endpoints. This mirrors
//! the composition `translate_audio` runs server-side for the cascade
//! engine, except transcription happens natively instead of over HTTP.
//!
//! Experimental and opt-in — see `engine::build`'s `LT_STT_BACKEND=native`
//! switch, which falls back to `PythonSidecarEngine` on any failure here.

use std::sync::Mutex;

use crate::translation::engine::{Decoded, DecodedWithAudio, TranslationEngine};
use crate::translation::engine_server::{self, LangPair};

/// Same "nothing to show" signal the Python sidecar uses (see
/// `pipeline.translate_audio`'s `ValueError("transcription produced no
/// text")`, surfaced to Rust as an error string `live.rs` matches on to skip
/// the segment silently instead of treating it as a real failure).
const NO_SPEECH_ERROR: &str = "transcription produced no text";

/// Engine that transcribes natively via `transcribe_cpp` and delegates MT +
/// TTS to the Python sidecar. `transcribe_cpp::Session::run` takes `&mut
/// self` and the type is `Send` but deliberately not `Sync` (single
/// in-flight decode per session) — the mutex below is what makes this safe
/// to share across the worker and partial-decode threads that call
/// `TranslationEngine` concurrently.
pub struct NativeCascadeEngine {
    session: Mutex<transcribe_cpp::Session>,
}

impl NativeCascadeEngine {
    /// Builds a session from an already-downloaded, verified, loaded model
    /// (see `models::manager::ModelManager::load`).
    pub fn new(model: transcribe_cpp::Model) -> Result<Self, String> {
        let session = model.session().map_err(|e| e.to_string())?;
        Ok(Self {
            session: Mutex::new(session),
        })
    }

    /// Runs the native ASR pass. Returns the raw (possibly empty/whitespace)
    /// transcript — callers decide what an empty result means for their case.
    fn transcribe(&self, samples: &[i16], lang: &LangPair) -> Result<String, String> {
        let pcm = i16_samples_to_f32(samples);
        let options = transcribe_cpp::RunOptions {
            task: transcribe_cpp::Task::Transcribe,
            language: Some(lang.src.clone()),
            ..Default::default()
        };
        let mut session = self
            .session
            .lock()
            .map_err(|_| "native STT session lock poisoned".to_string())?;
        let transcript = session.run(&pcm, &options).map_err(|e| e.to_string())?;
        Ok(transcript.text)
    }
}

impl TranslationEngine for NativeCascadeEngine {
    fn decode_and_synthesize(
        &self,
        samples: &[i16],
        use_cloned_voice: bool,
        lang: &LangPair,
    ) -> Result<DecodedWithAudio, String> {
        let source_text = self.transcribe(samples, lang)?;
        if source_text.trim().is_empty() {
            // Same semantics as the Python path: `live.rs`'s worker matches
            // this exact substring to skip the segment silently instead of
            // logging/keeping it as a genuine decode failure.
            return Err(NO_SPEECH_ERROR.to_string());
        }

        let translated_text = engine_server::mt(&source_text, lang)?;
        let output_wav = engine_server::tts(&translated_text, use_cloned_voice, &lang.tgt)?;

        Ok(DecodedWithAudio {
            decoded: Decoded {
                source_text,
                translated_text,
            },
            output_wav,
        })
    }

    fn decode_partial(&self, samples: &[i16], lang: &LangPair) -> Result<String, String> {
        let source_text = self.transcribe(samples, lang)?;
        if source_text.trim().is_empty() {
            // Partials use empty-string as the "nothing yet" signal (see
            // `pipeline.transcribe_translate`'s `if not text.strip(): return
            // ""`), not an error — `live.rs` just skips emitting on empty text.
            return Ok(String::new());
        }
        engine_server::mt(&source_text, lang)
    }
}

/// Converts 16-bit PCM samples to the `[-1, 1]` float range `transcribe_cpp`
/// expects.
fn i16_samples_to_f32(samples: &[i16]) -> Vec<f32> {
    samples.iter().map(|&s| s as f32 / 32768.0).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn i16_samples_to_f32_converts_zero() {
        assert_eq!(i16_samples_to_f32(&[0]), vec![0.0]);
    }

    #[test]
    fn i16_samples_to_f32_converts_full_scale_extremes() {
        let out = i16_samples_to_f32(&[i16::MAX, i16::MIN]);
        assert!((out[0] - (32767.0 / 32768.0)).abs() < 1e-6);
        assert_eq!(out[1], -1.0);
    }

    #[test]
    fn i16_samples_to_f32_preserves_length_and_order() {
        let samples: Vec<i16> = vec![0, 100, -100, 200, -200];
        let out = i16_samples_to_f32(&samples);
        assert_eq!(out.len(), samples.len());
        assert!(out[1] > 0.0);
        assert!(out[2] < 0.0);
        assert!(out[3] > out[1]);
    }

    #[test]
    fn no_speech_error_matches_the_string_live_rs_checks_for() {
        // live.rs's worker does `e.contains("transcription produced no
        // text")` to distinguish "no speech" from a real decode failure —
        // this pins that contract from this side too.
        assert!(NO_SPEECH_ERROR.contains("transcription produced no text"));
    }
}
