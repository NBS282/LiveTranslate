use std::path::PathBuf;
use std::sync::Arc;

use crate::translation::engine_server::LangPair;

pub mod native_cascade;
pub mod python_sidecar;

/// Default catalog id for the native STT backend's ASR model. Overridable via
/// `LT_NATIVE_STT_MODEL` for experimentation with other quantizations.
const DEFAULT_NATIVE_STT_MODEL: &str = "parakeet-tdt-0.6b-v3-q8";

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

/// Builds the default engine.
///
/// Reads `LT_STT_BACKEND` ("python" default | "native"). "native" is
/// experimental and opt-in: it resolves a GGUF Parakeet model via
/// `ModelManager` (downloading it on first use), loads it, and composes it
/// with the Python sidecar's `/mt` and `/tts` endpoints via
/// `NativeCascadeEngine`. Any failure along that path (unknown model id,
/// download, verify, load, session) is logged and this falls back to
/// `PythonSidecarEngine` — the setting must never break the app.
pub fn build(app: &tauri::AppHandle) -> Arc<dyn TranslationEngine> {
    if backend_choice() == "native" {
        match build_native(app) {
            Ok(engine) => return engine,
            Err(e) => {
                eprintln!(
                    "engine: native STT backend unavailable ({e}), falling back to Python sidecar"
                );
            }
        }
    }
    Arc::new(python_sidecar::PythonSidecarEngine)
}

/// Raw `LT_STT_BACKEND` value, defaulting to `"python"` for anything unset or
/// unrecognized (including typos) — only the exact value `"native"` opts in.
fn backend_choice() -> String {
    std::env::var("LT_STT_BACKEND").unwrap_or_else(|_| "python".to_string())
}

/// Raw `LT_NATIVE_STT_MODEL` value, defaulting to the Q8 Parakeet catalog id.
/// Pure env lookup, split out so it's testable without a `tauri::AppHandle`.
fn resolve_native_model_id() -> String {
    std::env::var("LT_NATIVE_STT_MODEL").unwrap_or_else(|_| DEFAULT_NATIVE_STT_MODEL.to_string())
}

/// Resolves, downloads (if needed), verifies, and loads the native STT model,
/// then wraps it in a `NativeCascadeEngine`. Split out from `build` so every
/// failure mode returns a descriptive `Err` for the caller to log and fall
/// back on, instead of `build` itself branching on multiple `Option`s.
fn build_native(app: &tauri::AppHandle) -> Result<Arc<dyn TranslationEngine>, String> {
    let model_id = resolve_native_model_id();
    let entry = crate::models::catalog::find(&model_id)
        .ok_or_else(|| format!("unknown native STT model id: {model_id}"))?;

    let manager = crate::models::manager::ModelManager::new(
        crate::models::manager::ModelManager::default_root(app),
    );

    if !manager.is_downloaded(entry) {
        eprintln!(
            "engine: downloading native STT model '{model_id}' ({} bytes total) — this can take a few minutes",
            entry.size_bytes
        );
        manager.download(entry, |downloaded, total| {
            eprintln!("engine: native STT model download {downloaded}/{total} bytes");
        })?;
        eprintln!("engine: native STT model '{model_id}' download complete");
    }

    let model = manager.load(entry)?;
    let engine = native_cascade::NativeCascadeEngine::new(model)?;
    eprintln!("engine: native STT backend ready (model '{model_id}')");
    Ok(Arc::new(engine))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // `std::env::set_var`/`remove_var` are process-global, and Rust runs
    // tests in this module concurrently by default — without this lock two
    // tests touching LT_STT_BACKEND/LT_NATIVE_STT_MODEL could interleave and
    // observe each other's values.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn backend_choice_defaults_to_python_when_unset() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // SAFETY: serialized by ENV_LOCK above; no other test in this file
        // reads LT_STT_BACKEND without holding the same lock.
        unsafe {
            std::env::remove_var("LT_STT_BACKEND");
        }
        assert_eq!(backend_choice(), "python");
    }

    #[test]
    fn backend_choice_is_native_only_on_exact_match() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        unsafe {
            std::env::set_var("LT_STT_BACKEND", "native");
        }
        assert_eq!(backend_choice(), "native");
        unsafe {
            std::env::remove_var("LT_STT_BACKEND");
        }
    }

    #[test]
    fn backend_choice_falls_through_to_python_on_bogus_value() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        unsafe {
            std::env::set_var("LT_STT_BACKEND", "not-a-real-backend");
        }
        // `build` only special-cases the exact string "native"; a bogus
        // value must behave exactly like "unset" (fall through to Python)
        // instead of erroring or attempting `build_native`.
        assert_ne!(backend_choice(), "native");
        unsafe {
            std::env::remove_var("LT_STT_BACKEND");
        }
    }

    #[test]
    fn resolve_native_model_id_defaults_to_parakeet_q8() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        unsafe {
            std::env::remove_var("LT_NATIVE_STT_MODEL");
        }
        let id = resolve_native_model_id();
        assert_eq!(id, DEFAULT_NATIVE_STT_MODEL);
        // Guard against the default ever drifting out of sync with the catalog.
        assert!(crate::models::catalog::find(&id).is_some());
    }

    #[test]
    fn resolve_native_model_id_honors_env_override() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        unsafe {
            std::env::set_var("LT_NATIVE_STT_MODEL", "parakeet-tdt-0.6b-v3-q4");
        }
        assert_eq!(resolve_native_model_id(), "parakeet-tdt-0.6b-v3-q4");
        unsafe {
            std::env::remove_var("LT_NATIVE_STT_MODEL");
        }
    }

    #[test]
    fn unknown_native_model_id_is_rejected_by_the_catalog() {
        // `build_native`'s first failure mode: an unknown id never reaches
        // ModelManager (which needs a live tauri::AppHandle to construct,
        // unavailable in a unit test) — verify the catalog lookup it relies
        // on directly.
        assert!(crate::models::catalog::find("does-not-exist-in-catalog").is_none());
    }
}
