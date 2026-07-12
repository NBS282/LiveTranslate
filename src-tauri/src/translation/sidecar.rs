use std::path::{Path, PathBuf};

/// Result of a successful offline translation.
#[derive(Debug, Clone, PartialEq)]
pub struct TranslationOutput {
    pub output_wav: PathBuf,
    pub source_text: String,
    pub translated_text: String,
}

/// Parses the engine's result.json content into (source_text, translated_text).
pub fn parse_result_json(s: &str) -> Result<(String, String), String> {
    let v: serde_json::Value = serde_json::from_str(s).map_err(|e| e.to_string())?;
    let src = v
        .get("source_text")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    let tgt = v
        .get("translated_text")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    Ok((src, tgt))
}

pub fn repo_root() -> PathBuf {
    // Explicit override — set by lib.rs at startup for packaged/installed builds.
    if let Ok(p) = std::env::var("LT_ENGINE_ROOT") {
        return PathBuf::from(p);
    }
    // Dev builds only: CARGO_MANIFEST_DIR is baked in at compile time and is
    // machine-specific. Never use it in release builds.
    #[cfg(debug_assertions)]
    {
        let compile_time = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."));
        if compile_time.exists() {
            return compile_time;
        }
    }
    // Packaged fallback: directory of the running executable.
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Relative path, under `<repo_root>/engine/python/`, of the production
/// portable Python interpreter for the given `std::env::consts::OS`-style
/// tag ("windows", "macos", ...).
///
/// Pure function (no filesystem access) so both branches are exercised by
/// unit tests regardless of which OS actually runs them — a typo here would
/// otherwise only surface on the other platform's CI job.
///
/// - Windows: `python-build-standalone`'s `install_only` archive is renamed
///   to `livetranslate-engine.exe` by `setup::download_portable_python` so
///   Task Manager shows the app name instead of "python.exe".
/// - Everything else (macOS today; matches python-build-standalone's Unix
///   `install_only` layout): the interpreter lives at `bin/python3`, a
///   symlink to the pinned `python3.NN` binary. No renaming applies there.
pub fn production_python_rel_path(os: &str) -> PathBuf {
    if os == "windows" {
        PathBuf::from("livetranslate-engine.exe")
    } else {
        PathBuf::from("bin").join("python3")
    }
}

/// Python interpreter used by the engine. Override with LT_ENGINE_PYTHON.
///
/// Resolution order:
///   1. LT_ENGINE_PYTHON env var (manual override)
///   2. engine/python/<production_python_rel_path>  (production portable runtime)
///   3. .venv-engine/Scripts|bin/python[.exe]        (dev venv fallback)
pub fn engine_python() -> String {
    if let Ok(p) = std::env::var("LT_ENGINE_PYTHON") {
        return p;
    }
    // Production: portable Python-build-standalone runtime downloaded by setup.
    let prod_exe = repo_root()
        .join("engine")
        .join("python")
        .join(production_python_rel_path(std::env::consts::OS));
    if prod_exe.exists() {
        return prod_exe.to_string_lossy().into_owned();
    }
    // Dev fallback: venv created by setup.
    let rel = if cfg!(windows) {
        ".venv-engine/Scripts/python.exe"
    } else {
        ".venv-engine/bin/python"
    };
    repo_root().join(rel).to_string_lossy().into_owned()
}

/// Directory containing the `lt_engine` package (repo `python/`), used as cwd so `-m lt_engine` resolves.
pub fn python_dir() -> PathBuf {
    repo_root().join("python")
}

/// Translates `input` to English audio + text via the persistent translation server.
pub fn translate_file(input: &Path) -> Result<TranslationOutput, String> {
    crate::translation::engine_server::translate(input)
}

#[cfg(test)]
mod json_tests {
    use super::*;

    #[test]
    fn production_python_rel_path_windows_is_renamed_exe() {
        assert_eq!(
            production_python_rel_path("windows"),
            PathBuf::from("livetranslate-engine.exe")
        );
    }

    #[test]
    fn production_python_rel_path_macos_is_bin_python3() {
        assert_eq!(
            production_python_rel_path("macos"),
            PathBuf::from("bin").join("python3")
        );
    }

    #[test]
    fn production_python_rel_path_defaults_to_unix_layout() {
        // Anything that isn't "windows" follows python-build-standalone's
        // Unix `install_only` layout (bin/python3) — covers macOS today and
        // keeps the fallback sane if Linux is ever supported.
        assert_eq!(
            production_python_rel_path("linux"),
            PathBuf::from("bin").join("python3")
        );
    }

    #[test]
    fn parses_both_texts() {
        let (s, t) =
            parse_result_json(r#"{"source_text":"hola","translated_text":"hello"}"#).unwrap();
        assert_eq!(s, "hola");
        assert_eq!(t, "hello");
    }

    #[test]
    fn missing_fields_default_empty() {
        let (s, t) = parse_result_json("{}").unwrap();
        assert_eq!(s, "");
        assert_eq!(t, "");
    }

    #[test]
    fn invalid_json_errors() {
        assert!(parse_result_json("not json").is_err());
    }
}
