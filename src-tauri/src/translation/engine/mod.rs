use std::path::PathBuf;
use std::sync::Arc;

use crate::translation::engine_server::LangPair;

pub mod python_sidecar;

/// Text-only result of a decode. `source_text` is empty for engines that only
/// perform AST (audio-to-speech-translation) without a separate transcript,
/// e.g. Canary via the current Python sidecar.
pub struct Decoded {
    pub source_text: String,
    pub translated_text: String,
}

/// Full result of a closed-segment decode: text plus the synthesized reply
/// audio. Phase 1 only — the trait split becomes meaningful once native
/// engines (e.g. transcribe.cpp) arrive and text/audio synthesis may come
/// from different backends.
pub struct DecodedWithAudio {
    pub decoded: Decoded,
    pub output_wav: PathBuf,
}

/// Abstraction over "turn captured mic audio into translated speech + text".
/// Today the only implementation (`python_sidecar::PythonSidecarEngine`)
/// wraps the existing HTTP calls to the Python sidecar. The trait exists so a
/// future native engine can be swapped in without touching call sites in
/// `live.rs` — this is a pure seam, not a behavior change.
pub trait TranslationEngine: Send + Sync {
    /// Full decode + synthesis of a closed segment (16 kHz mono i16 samples).
    fn decode_and_synthesize(
        &self,
        samples: &[i16],
        use_cloned_voice: bool,
        lang: &LangPair,
    ) -> Result<DecodedWithAudio, String>;

    /// Display-only partial for live subtitles (latency-sensitive, no audio).
    fn decode_partial(&self, samples: &[i16], lang: &LangPair) -> Result<String, String>;
}

/// Builds the default engine. Phase 1: always the Python sidecar — there is
/// no config/env switch yet, this is purely the seam for one to be added later.
pub fn build() -> Arc<dyn TranslationEngine> {
    Arc::new(python_sidecar::PythonSidecarEngine)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_returns_a_usable_trait_object() {
        let engine = build();
        // Compile-time proof the returned Arc satisfies the trait's Send +
        // Sync + dyn-compatible bounds; there is no pure-logic behavior to
        // assert beyond that without hitting the live HTTP server.
        let _: &dyn TranslationEngine = &*engine;
    }
}
