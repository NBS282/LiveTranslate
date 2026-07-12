use std::path::PathBuf;
use std::sync::Arc;

use crate::translation::engine_server::LangPair;

pub mod native_canary;
pub mod native_cascade;
pub mod python_sidecar;

/// Default catalog id for the native STT backend's ASR model. Overridable via
/// `LT_NATIVE_STT_MODEL` for experimentation with other quantizations.
const DEFAULT_NATIVE_STT_MODEL: &str = "parakeet-tdt-0.6b-v3-q8";

/// Default catalog id for the native Canary AST engine's model. Overridable
/// via `LT_NATIVE_AST_MODEL` for experimentation with other quantizations.
const DEFAULT_NATIVE_AST_MODEL: &str = "canary-1b-flash-q8";

/// Converts 16-bit PCM samples to the `[-1, 1]` float range `transcribe_cpp`
/// expects. Shared by every native engine (`native_cascade`, `native_canary`)
/// so the conversion lives in one place instead of being duplicated per file.
fn i16_samples_to_f32(samples: &[i16]) -> Vec<f32> {
    samples.iter().map(|&s| s as f32 / 32768.0).collect()
}

/// Text-only result of a decode. `source_text` is empty for engines that only
/// perform AST (audio-to-speech-translation) without a separate transcript,
/// e.g. the native Canary engine (`native_canary::NativeCanaryEngine`), which
/// runs `transcribe_cpp` with `Task::Translate` and never produces an
/// intermediate source-language transcript.
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
/// Reads `LT_TRANSLATION_ENGINE` ("cascade" default | "canary") first: the
/// exact value "canary" opts into the native Canary AST engine, which takes
/// priority over `LT_STT_BACKEND` regardless of its value. Any failure
/// resolving/loading the Canary model falls back straight to
/// `PythonSidecarEngine` — after the Python NeMo Canary path was removed,
/// that sidecar only ever runs the cascade pipeline, so a Canary failure
/// degrades to cascade rather than retrying the native cascade engine.
///
/// Otherwise reads `LT_STT_BACKEND` ("native" default | "python"). "native"
/// resolves a GGUF Parakeet model via `ModelManager` (downloading it on
/// first use), loads it, and composes it with the Python sidecar's `/mt`
/// and `/tts` endpoints via `NativeCascadeEngine`. Any failure along that
/// path (unknown model id, download, verify, load, session) is logged and
/// this falls back to `PythonSidecarEngine` — the setting must never break
/// the app. "python" is the explicit opt-out, for machines that already
/// have `nemo_toolkit` installed and want the legacy cascade pipeline.
///
/// Note: `engine_server::spawn_server` sets `LT_STT_BACKEND` on the Python
/// sidecar's environment to this same resolved value (see
/// `resolved_stt_backend`) so both processes agree — but the sidecar is
/// spawned *before* this function runs (at app startup / `warm_engine`),
/// so a native failure discovered here falls back to `PythonSidecarEngine`
/// for a sidecar that was already told "native" and therefore skipped its
/// eager NeMo ASR warmup. That fallback session still works: `pipeline.py`
/// lazy-loads NeMo ASR on the first request in that case, just slower for
/// that one segment.
pub fn build(app: &tauri::AppHandle) -> Arc<dyn TranslationEngine> {
    if translation_engine_choice() == "canary" {
        return match build_native_canary(app) {
            Ok(engine) => engine,
            Err(e) => {
                eprintln!(
                    "engine: native Canary AST engine unavailable ({e}), falling back to Python sidecar (cascade pipeline — Canary is not available there)"
                );
                Arc::new(python_sidecar::PythonSidecarEngine)
            }
        };
    }

    if backend_choice() != "python" {
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

/// Raw `LT_TRANSLATION_ENGINE` value, defaulting to `"cascade"` for anything
/// unset or unrecognized — only the exact value `"canary"` opts into the
/// native Canary AST engine.
fn translation_engine_choice() -> String {
    std::env::var("LT_TRANSLATION_ENGINE").unwrap_or_else(|_| "cascade".to_string())
}

/// Raw `LT_NATIVE_AST_MODEL` value, defaulting to the Q8 Canary catalog id.
/// Pure env lookup, split out so it's testable without a `tauri::AppHandle`.
fn resolve_native_ast_model_id() -> String {
    std::env::var("LT_NATIVE_AST_MODEL").unwrap_or_else(|_| DEFAULT_NATIVE_AST_MODEL.to_string())
}

/// Resolves, downloads (if needed), verifies, and loads the native Canary AST
/// model, then wraps it in a `NativeCanaryEngine`. Split out from `build` so
/// every failure mode returns a descriptive `Err` for the caller to log and
/// fall back on, instead of `build` itself branching on multiple `Option`s.
fn build_native_canary(app: &tauri::AppHandle) -> Result<Arc<dyn TranslationEngine>, String> {
    let model_id = resolve_native_ast_model_id();
    let entry = crate::models::catalog::find(&model_id)
        .ok_or_else(|| format!("unknown native AST model id: {model_id}"))?;

    let manager = crate::models::manager::ModelManager::new(
        crate::models::manager::ModelManager::default_root(app),
    );

    if !manager.is_downloaded(entry) {
        eprintln!(
            "engine: downloading native Canary AST model '{model_id}' ({} bytes total) — this can take a few minutes",
            entry.size_bytes
        );
        manager.download(entry, |downloaded, total| {
            eprintln!("engine: native Canary AST model download {downloaded}/{total} bytes");
        })?;
        eprintln!("engine: native Canary AST model '{model_id}' download complete");
    }

    let model = manager.load(entry)?;
    let engine = native_canary::NativeCanaryEngine::new(model)?;
    eprintln!("engine: native Canary AST engine ready (model '{model_id}')");
    Ok(Arc::new(engine))
}

/// Raw `LT_STT_BACKEND` value, defaulting to `"native"` for anything unset or
/// unrecognized (including typos) — only the exact value `"python"` opts out
/// back to the Python sidecar's NeMo Parakeet ASR path.
///
/// `pub(crate)` so `engine_server::spawn_server` can propagate this exact
/// resolution onto the Python sidecar's environment — see `resolved_stt_backend`.
fn backend_choice() -> String {
    std::env::var("LT_STT_BACKEND").unwrap_or_else(|_| "native".to_string())
}

/// Resolved `LT_STT_BACKEND` choice, shared with `engine_server::spawn_server`
/// so the Rust engine selection and the Python sidecar's warmup both agree on
/// whether native STT is primary — without either side duplicating the env
/// lookup or its default.
pub(crate) fn resolved_stt_backend() -> String {
    backend_choice()
}

/// Raw `LT_NATIVE_STT_MODEL` value, defaulting to the Q8 Parakeet catalog id.
/// Pure env lookup, split out so it's testable without a `tauri::AppHandle`.
///
/// `pub(crate)` so `setup::download_native_stt_model` resolves the exact
/// same catalog id this module downloads on-demand — one source of truth
/// for "which model id is the native STT default".
pub(crate) fn resolve_native_model_id() -> String {
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
    fn backend_choice_defaults_to_native_when_unset() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // SAFETY: serialized by ENV_LOCK above; no other test in this file
        // reads LT_STT_BACKEND without holding the same lock.
        unsafe {
            std::env::remove_var("LT_STT_BACKEND");
        }
        assert_eq!(backend_choice(), "native");
    }

    #[test]
    fn backend_choice_is_python_only_on_exact_match() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        unsafe {
            std::env::set_var("LT_STT_BACKEND", "python");
        }
        assert_eq!(backend_choice(), "python");
        unsafe {
            std::env::remove_var("LT_STT_BACKEND");
        }
    }

    #[test]
    fn backend_choice_falls_through_to_native_on_bogus_value() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        unsafe {
            std::env::set_var("LT_STT_BACKEND", "not-a-real-backend");
        }
        // `build` only special-cases the exact string "python" as an
        // opt-out; a bogus value must behave exactly like "unset" (native)
        // instead of erroring or opting out of `build_native`.
        assert_ne!(backend_choice(), "python");
        unsafe {
            std::env::remove_var("LT_STT_BACKEND");
        }
    }

    #[test]
    fn resolved_stt_backend_matches_backend_choice() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        unsafe {
            std::env::set_var("LT_STT_BACKEND", "python");
        }
        // `engine_server::spawn_server` propagates this exact value onto the
        // Python sidecar's environment — pin that it never drifts from what
        // `build` itself reads.
        assert_eq!(resolved_stt_backend(), backend_choice());
        unsafe {
            std::env::remove_var("LT_STT_BACKEND");
        }
        assert_eq!(resolved_stt_backend(), "native");
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

    #[test]
    fn translation_engine_choice_defaults_to_cascade_when_unset() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        unsafe {
            std::env::remove_var("LT_TRANSLATION_ENGINE");
        }
        assert_eq!(translation_engine_choice(), "cascade");
    }

    #[test]
    fn translation_engine_choice_is_canary_only_on_exact_match() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        unsafe {
            std::env::set_var("LT_TRANSLATION_ENGINE", "canary");
        }
        assert_eq!(translation_engine_choice(), "canary");
        unsafe {
            std::env::remove_var("LT_TRANSLATION_ENGINE");
        }
    }

    #[test]
    fn translation_engine_choice_falls_through_to_cascade_on_bogus_value() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        unsafe {
            std::env::set_var("LT_TRANSLATION_ENGINE", "not-a-real-engine");
        }
        // Only the exact string "canary" opts in; a bogus value must behave
        // exactly like "unset" (fall through to cascade) instead of erroring.
        assert_ne!(translation_engine_choice(), "canary");
        unsafe {
            std::env::remove_var("LT_TRANSLATION_ENGINE");
        }
    }

    #[test]
    fn canary_selection_beats_stt_backend_regardless_of_its_value() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        unsafe {
            std::env::set_var("LT_TRANSLATION_ENGINE", "canary");
            std::env::set_var("LT_STT_BACKEND", "native");
        }
        // `build` checks translation_engine_choice() before backend_choice() —
        // pin that ordering here so a future refactor can't silently swap it.
        assert_eq!(translation_engine_choice(), "canary");
        assert_eq!(backend_choice(), "native");
        unsafe {
            std::env::remove_var("LT_TRANSLATION_ENGINE");
            std::env::remove_var("LT_STT_BACKEND");
        }
    }

    #[test]
    fn resolve_native_ast_model_id_defaults_to_canary_q8() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        unsafe {
            std::env::remove_var("LT_NATIVE_AST_MODEL");
        }
        let id = resolve_native_ast_model_id();
        assert_eq!(id, DEFAULT_NATIVE_AST_MODEL);
        // Guard against the default ever drifting out of sync with the catalog.
        assert!(crate::models::catalog::find(&id).is_some());
    }

    #[test]
    fn resolve_native_ast_model_id_honors_env_override() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        unsafe {
            std::env::set_var("LT_NATIVE_AST_MODEL", "canary-1b-flash-q4");
        }
        assert_eq!(resolve_native_ast_model_id(), "canary-1b-flash-q4");
        unsafe {
            std::env::remove_var("LT_NATIVE_AST_MODEL");
        }
    }

    #[test]
    fn unknown_native_ast_model_id_is_rejected_by_the_catalog() {
        // `build_native_canary`'s first failure mode: an unknown id never
        // reaches ModelManager (which needs a live tauri::AppHandle to
        // construct, unavailable in a unit test) — verify the catalog lookup
        // it relies on directly.
        assert!(crate::models::catalog::find("does-not-exist-in-catalog").is_none());
    }

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
}
