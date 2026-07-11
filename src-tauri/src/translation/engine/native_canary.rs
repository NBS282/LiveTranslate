//! Native (in-process) Canary AST engine: `transcribe_cpp` runs Canary 1B
//! Flash (GGUF) directly with `Task::Translate`, producing translated text
//! from source-language audio in a single pass — there is no intermediate
//! source-language transcript, unlike the cascade engines. TTS is still
//! delegated to the Python sidecar's `/tts` endpoint, the same tail
//! `translate_audio` runs after translation in the Python cascade path.
//!
//! Experimental and opt-in — see `engine::build`'s
//! `LT_TRANSLATION_ENGINE=canary` switch, which falls back to
//! `PythonSidecarEngine` (running the cascade pipeline; the Python NeMo
//! Canary path was removed) on any failure here.

use std::sync::Mutex;

use crate::translation::engine::{
    i16_samples_to_f32, Decoded, DecodedWithAudio, TranslationEngine,
};
use crate::translation::engine_server::{self, LangPair};

/// Same "nothing to show" signal the Python cascade path uses (see
/// `pipeline.translate_audio`'s `ValueError("transcription produced no
/// text")`), surfaced to Rust as an error string `live.rs` matches on to skip
/// the segment silently instead of treating it as a real failure.
const NO_SPEECH_ERROR: &str = "transcription produced no text";

/// Engine that runs Canary 1B Flash AST natively via `transcribe_cpp` and
/// delegates only synthesis to the Python sidecar — there is no MT step,
/// Canary emits translated text directly. See `NativeCascadeEngine` for the
/// mutex/poisoned-lock recovery rationale, which applies identically here:
/// `transcribe_cpp::Session::run` takes `&mut self` and the type is `Send`
/// but deliberately not `Sync` (single in-flight decode per session).
pub struct NativeCanaryEngine {
    session: Mutex<transcribe_cpp::Session>,
}

impl NativeCanaryEngine {
    /// Builds a session from an already-downloaded, verified, loaded model
    /// (see `models::manager::ModelManager::load`).
    pub fn new(model: transcribe_cpp::Model) -> Result<Self, String> {
        let session = model.session().map_err(|e| e.to_string())?;
        Ok(Self {
            session: Mutex::new(session),
        })
    }

    /// Runs the native AST pass. Returns the raw (possibly empty/whitespace)
    /// translated text — callers decide what an empty result means for their
    /// case.
    fn translate(&self, samples: &[i16], lang: &LangPair) -> Result<String, String> {
        let pcm = i16_samples_to_f32(samples);
        let options = transcribe_cpp::RunOptions {
            task: transcribe_cpp::Task::Translate,
            language: Some(lang.src.clone()),
            target_language: Some(lang.tgt.clone()),
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

impl TranslationEngine for NativeCanaryEngine {
    fn decode_and_synthesize(
        &self,
        samples: &[i16],
        use_cloned_voice: bool,
        lang: &LangPair,
    ) -> Result<DecodedWithAudio, String> {
        let translated_text = self.translate(samples, lang)?;
        if translated_text.trim().is_empty() {
            // Same semantics as the Python cascade path: `live.rs`'s worker
            // matches this exact substring to skip the segment silently
            // instead of logging/keeping it as a genuine decode failure.
            return Err(NO_SPEECH_ERROR.to_string());
        }

        let output_wav = engine_server::tts(&translated_text, use_cloned_voice, &lang.tgt)?;

        Ok(DecodedWithAudio {
            decoded: Decoded {
                // Canary AST never produces an intermediate source-language
                // transcript — `live.rs` already treats an empty
                // `source_text` as this engine's signature (see its
                // echo-mitigation comment) rather than a bug.
                source_text: String::new(),
                translated_text,
            },
            output_wav,
        })
    }

    fn decode_partial(&self, samples: &[i16], lang: &LangPair) -> Result<String, String> {
        let translated_text = self.translate(samples, lang)?;
        if translated_text.trim().is_empty() {
            // Partials use empty-string as the "nothing yet" signal (same
            // contract as the cascade engines), not an error — `live.rs`
            // just skips emitting on empty text.
            return Ok(String::new());
        }
        Ok(translated_text)
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
