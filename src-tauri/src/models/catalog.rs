//! Static catalog of GGUF models available for native speech-to-text (ASR) and
//! speech-to-text-translation (AST). Entries are hand-verified against the
//! Hugging Face Hub (repo, filename, size, sha256) so `ModelManager` can check
//! a download without trusting the server's byte count alone.

/// GGUF quantization level.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Quant {
    F16,
    F32,
    Q4KM,
    Q5KM,
    Q6K,
    Q8_0,
}

/// What a model is used for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Task {
    /// Automatic speech recognition: transcribe in the source language.
    Asr,
    /// Automatic speech translation: source-language audio to target-language text.
    Ast,
}

/// A single catalog entry: everything needed to locate, download, and verify
/// one GGUF model file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelEntry {
    pub id: &'static str,
    pub hf_repo: &'static str,
    pub filename: &'static str,
    pub quant: Quant,
    pub size_bytes: u64,
    pub sha256: &'static str,
    pub task: Task,
    /// Expected GGUF `general.architecture` metadata value, e.g. "parakeet".
    pub arch: &'static str,
}

impl ModelEntry {
    /// Direct download URL for this entry's file on the Hugging Face Hub.
    pub fn download_url(&self) -> String {
        format!(
            "https://huggingface.co/{}/resolve/main/{}",
            self.hf_repo, self.filename
        )
    }
}

pub const CATALOG: &[ModelEntry] = &[
    ModelEntry {
        id: "parakeet-tdt-0.6b-v3-q8",
        hf_repo: "handy-computer/parakeet-tdt-0.6b-v3-gguf",
        filename: "parakeet-tdt-0.6b-v3-Q8_0.gguf",
        quant: Quant::Q8_0,
        size_bytes: 739_508_576,
        sha256: "5859f77944efcd8eafa23a6350731960b2b55b2203df51f319665c807d802cc7",
        task: Task::Asr,
        arch: "parakeet",
    },
    ModelEntry {
        id: "parakeet-tdt-0.6b-v3-q4",
        hf_repo: "handy-computer/parakeet-tdt-0.6b-v3-gguf",
        filename: "parakeet-tdt-0.6b-v3-Q4_K_M.gguf",
        quant: Quant::Q4KM,
        size_bytes: 485_425_504,
        sha256: "b68557be1e3c40207fd7c4bd9d63f1d3316b963f15325bfb0cc16a8bb0ffd181",
        task: Task::Asr,
        arch: "parakeet",
    },
    ModelEntry {
        id: "canary-1b-flash-q8",
        hf_repo: "handy-computer/canary-1b-flash-gguf",
        filename: "canary-1b-flash-Q8_0.gguf",
        quant: Quant::Q8_0,
        size_bytes: 1_048_131_360,
        sha256: "9b99e0881d883467e0a03ceb0968dba888c9f8921b73355143ad8b67931e08ce",
        task: Task::Ast,
        arch: "canary",
    },
    ModelEntry {
        id: "canary-1b-flash-q4",
        hf_repo: "handy-computer/canary-1b-flash-gguf",
        filename: "canary-1b-flash-Q4_K_M.gguf",
        quant: Quant::Q4KM,
        size_bytes: 677_141_280,
        sha256: "2521a615a2b04ab11900d894e1f5c5a70405d85aa74be0806b569b9cc311a707",
        task: Task::Ast,
        arch: "canary",
    },
];

/// Looks up a catalog entry by id.
pub fn find(id: &str) -> Option<&'static ModelEntry> {
    CATALOG.iter().find(|e| e.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_returns_known_entry() {
        let entry = find("parakeet-tdt-0.6b-v3-q8").expect("entry should exist");
        assert_eq!(entry.hf_repo, "handy-computer/parakeet-tdt-0.6b-v3-gguf");
        assert_eq!(entry.task, Task::Asr);
    }

    #[test]
    fn find_returns_none_for_unknown_id() {
        assert!(find("does-not-exist").is_none());
    }

    #[test]
    fn download_url_builds_expected_hf_resolve_link() {
        let entry = find("canary-1b-flash-q4").expect("entry should exist");
        assert_eq!(
            entry.download_url(),
            "https://huggingface.co/handy-computer/canary-1b-flash-gguf/resolve/main/canary-1b-flash-Q4_K_M.gguf"
        );
    }

    #[test]
    fn catalog_ids_are_unique() {
        let mut ids: Vec<&str> = CATALOG.iter().map(|e| e.id).collect();
        let original_len = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), original_len, "duplicate id in CATALOG");
    }
}
