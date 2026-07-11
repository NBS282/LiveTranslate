//! Native (in-process) STT cascade engine: `transcribe_cpp` GGUF ASR runs
//! locally, then MarianMT translation and Piper/Pocket-TTS synthesis are
//! delegated to the Python sidecar's `/mt` and `/tts` endpoints. This mirrors
//! the composition `translate_audio` runs server-side for the cascade
//! engine, except transcription happens natively instead of over HTTP.
//!
//! Experimental and opt-in — see `engine::build`'s `LT_STT_BACKEND=native`
//! switch, which falls back to `PythonSidecarEngine` on any failure here.

use std::sync::Mutex;

use crate::translation::engine::{
    i16_samples_to_f32, Decoded, DecodedWithAudio, TranslationEngine,
};
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
        // Recover a poisoned lock instead of failing every later segment: a
        // panic mid-decode leaves no partial state we rely on (the next run()
        // starts a fresh utterance), and the transcribe-cpp crate recovers its
        // internal compute lock the same way.
        let mut session = self
            .session
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_speech_error_matches_the_string_live_rs_checks_for() {
        // live.rs's worker does `e.contains("transcription produced no
        // text")` to distinguish "no speech" from a real decode failure —
        // this pins that contract from this side too.
        assert!(NO_SPEECH_ERROR.contains("transcription produced no text"));
    }
}
