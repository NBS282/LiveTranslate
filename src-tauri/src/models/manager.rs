//! Downloads, verifies, and loads GGUF models from the catalog.
//!
//! Layout on disk: `<root>/models/gguf/<id>/<filename>`, kept alongside the
//! existing `<root>/models` used for the Piper TTS voice (see `setup.rs`) so
//! all model assets live under one directory that survives app updates.
//!
//! No load/unload registry lives here yet — `load` hands the caller an owned
//! `transcribe_cpp::Model` and lifecycle policy (caching, eviction) is left to
//! Phase 3, which is the first phase with real callers.

use std::io::Read;
use std::path::PathBuf;

use sha2::{Digest, Sha256};

use super::catalog::ModelEntry;
use super::gguf_meta;
use crate::net::download;

pub struct ModelManager {
    root: PathBuf,
}

impl ModelManager {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    /// Resolves the same base directory `setup::check_with_app` uses for
    /// `LT_ENGINE_ROOT`, so downloaded GGUF models live under
    /// `<root>/models/gguf/` alongside `<root>/models/hf` (Python engine
    /// cache) and `<root>/models/*.onnx` (Piper voice).
    ///
    /// Mirrors `check_with_app`'s pattern of resolving straight from the app's
    /// data dir in release builds, instead of trusting a possibly-not-yet-set
    /// `LT_ENGINE_ROOT` env var.
    pub fn default_root(app: &tauri::AppHandle) -> PathBuf {
        #[cfg(not(debug_assertions))]
        {
            use tauri::Manager;
            if let Ok(dir) = app.path().app_local_data_dir() {
                return dir;
            }
        }
        let _ = app; // only used in release builds above
        crate::translation::sidecar::repo_root()
    }

    /// Where `entry`'s GGUF file lives (whether downloaded yet or not).
    pub fn path_for(&self, entry: &ModelEntry) -> PathBuf {
        self.root
            .join("models")
            .join("gguf")
            .join(entry.id)
            .join(entry.filename)
    }

    /// True when the file exists and its size matches the catalog entry.
    /// A cheap check, not a substitute for `verify` (sha256 + GGUF arch).
    pub fn is_downloaded(&self, entry: &ModelEntry) -> bool {
        std::fs::metadata(self.path_for(entry))
            .map(|meta| meta.len() == entry.size_bytes)
            .unwrap_or(false)
    }

    /// Downloads `entry`'s file, resuming a partial download if one exists.
    /// `progress(downloaded_bytes, total_bytes)` is called as bytes arrive;
    /// `total_bytes` is 0 when the server didn't report a size.
    pub fn download(
        &self,
        entry: &ModelEntry,
        progress: impl Fn(u64, u64),
    ) -> Result<PathBuf, String> {
        let dest = self.path_for(entry);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }

        download::download_with_progress(
            &entry.download_url(),
            &dest,
            download::DEFAULT_MAX_ATTEMPTS,
            |_attempt| {},
            progress,
        )?;

        Ok(dest)
    }

    /// Verifies the downloaded file's sha256 and GGUF `general.architecture`
    /// against the catalog entry. Streams the file rather than loading it
    /// wholesale — these models run into the hundreds of MB to ~1 GB.
    pub fn verify(&self, entry: &ModelEntry) -> Result<(), String> {
        let path = self.path_for(entry);

        let mut file = std::fs::File::open(&path)
            .map_err(|e| format!("failed to open {}: {e}", path.display()))?;
        let mut hasher = Sha256::new();
        let mut buf = [0u8; 1 << 16];
        loop {
            let n = file
                .read(&mut buf)
                .map_err(|e| format!("failed to read {}: {e}", path.display()))?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
        }
        let digest = format!("{:x}", hasher.finalize());
        if digest != entry.sha256 {
            return Err(format!(
                "sha256 mismatch for {}: expected {}, got {digest}",
                entry.filename, entry.sha256
            ));
        }

        match gguf_meta::read_architecture(&path) {
            Ok(Some(arch)) if arch == entry.arch => Ok(()),
            Ok(Some(arch)) => Err(format!(
                "unexpected GGUF architecture for {}: expected '{}', got '{arch}'",
                entry.filename, entry.arch
            )),
            Ok(None) => Err(format!(
                "could not read GGUF architecture metadata from {}",
                path.display()
            )),
            Err(e) => Err(format!(
                "failed to parse GGUF header for {}: {e}",
                entry.filename
            )),
        }
    }

    /// Verifies the downloaded file, then loads it into a `transcribe_cpp::Model`.
    pub fn load(&self, entry: &ModelEntry) -> Result<transcribe_cpp::Model, String> {
        self.verify(entry)?;
        transcribe_cpp::Model::load(self.path_for(entry)).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::catalog::{Quant, Task};

    const TEST_ENTRY: ModelEntry = ModelEntry {
        id: "test-model",
        hf_repo: "example/repo",
        filename: "test-model.gguf",
        quant: Quant::Q8_0,
        size_bytes: 5,
        sha256: "test-hash-not-a-real-sha256",
        task: Task::Asr,
        arch: "test-arch",
    };

    /// A fresh manager rooted at a uniquely-named temp dir, and that dir's
    /// path for cleanup. Callers must remove it when done.
    fn manager_in(unique_name: &str) -> (ModelManager, PathBuf) {
        let root = std::env::temp_dir().join(format!(
            "lt_model_manager_test_{unique_name}_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        (ModelManager::new(root.clone()), root)
    }

    #[test]
    fn path_for_uses_models_gguf_id_filename_layout() {
        let (manager, root) = manager_in("path_for");
        let path = manager.path_for(&TEST_ENTRY);
        assert_eq!(
            path,
            root.join("models")
                .join("gguf")
                .join("test-model")
                .join("test-model.gguf")
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn is_downloaded_false_when_file_missing() {
        let (manager, root) = manager_in("missing");
        assert!(!manager.is_downloaded(&TEST_ENTRY));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn is_downloaded_false_on_size_mismatch() {
        let (manager, root) = manager_in("size_mismatch");
        let path = manager.path_for(&TEST_ENTRY);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"ab").unwrap(); // 2 bytes; entry expects 5
        assert!(!manager.is_downloaded(&TEST_ENTRY));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn is_downloaded_true_when_size_matches() {
        let (manager, root) = manager_in("size_match");
        let path = manager.path_for(&TEST_ENTRY);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"abcde").unwrap(); // 5 bytes, matches entry
        assert!(manager.is_downloaded(&TEST_ENTRY));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn verify_fails_on_corrupted_file_with_wrong_hash() {
        let (manager, root) = manager_in("verify_corrupt");
        let path = manager.path_for(&TEST_ENTRY);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"abcde").unwrap();

        let result = manager.verify(&TEST_ENTRY);
        let err = result.expect_err("hash mismatch must be rejected");
        assert!(err.contains("sha256 mismatch"), "unexpected error: {err}");

        let _ = std::fs::remove_dir_all(&root);
    }
}
