use std::path::{Path, PathBuf};
use std::process::Command;

/// Result of a successful offline translation.
#[derive(Debug, Clone, PartialEq)]
pub struct TranslationOutput {
    pub output_wav: PathBuf,
    pub text: String,
}

fn has_ext(p: &Path, ext: &str) -> bool {
    p.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case(ext))
        .unwrap_or(false)
}

fn name_contains(p: &Path, needle: &str) -> bool {
    p.file_name()
        .and_then(|n| n.to_str())
        .map(|n| n.to_ascii_lowercase().contains(needle))
        .unwrap_or(false)
}

/// Picks the translated audio file from the files produced in the work dir.
/// Hibiki-Zero emits both a `_stereo.wav` and a `_mono.wav`; prefer stereo,
/// then mono, then any `.wav`.
pub fn pick_output_wav(files: &[PathBuf]) -> Option<PathBuf> {
    let wavs: Vec<&PathBuf> = files.iter().filter(|p| has_ext(p, "wav")).collect();
    wavs.iter()
        .find(|p| name_contains(p, "stereo"))
        .or_else(|| wavs.iter().find(|p| name_contains(p, "mono")))
        .or_else(|| wavs.first())
        .map(|p| (*p).clone())
}

/// Reads the translated text from the `.txt` file Hibiki-Zero writes alongside
/// the audio (the text is NOT printed to stdout). Empty string if none found.
pub fn read_translated_text(files: &[PathBuf]) -> String {
    files
        .iter()
        .find(|p| has_ext(p, "txt"))
        .and_then(|p| std::fs::read_to_string(p).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

/// Path to the hibiki-zero executable. Override with HIBIKI_ZERO_BIN; otherwise
/// resolves the project-local venv against the repo root (parent of src-tauri).
pub fn hibiki_zero_bin() -> String {
    resolve_hibiki_zero_bin(std::env::var("HIBIKI_ZERO_BIN").ok())
}

fn resolve_hibiki_zero_bin(override_val: Option<String>) -> String {
    if let Some(p) = override_val {
        return p;
    }
    // CARGO_MANIFEST_DIR is the src-tauri/ dir; the repo root is its parent.
    let repo_root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    let rel = if cfg!(windows) {
        ".venv-hibiki/Scripts/hibiki-zero.exe"
    } else {
        ".venv-hibiki/bin/hibiki-zero"
    };
    repo_root.join(rel).to_string_lossy().into_owned()
}

/// Builds the (program, args) to translate `input`, writing output into `out_dir`.
pub fn build_command(input: &Path, out_dir: &Path) -> (String, Vec<String>) {
    (
        hibiki_zero_bin(),
        vec![
            "generate".to_string(),
            "--file".to_string(),
            input.to_string_lossy().into_owned(),
            "--out-dir".to_string(),
            out_dir.to_string_lossy().into_owned(),
            "--bf16".to_string(),
        ],
    )
}

/// Translates `input` to English audio + text via the Hibiki-Zero sidecar.
/// Writes output into a fresh temp dir and locates the produced wav.
pub fn translate_file(input: &Path) -> Result<TranslationOutput, String> {
    if !input.exists() {
        return Err(format!("input file not found: {}", input.display()));
    }

    let ext_ok = input
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| {
            matches!(
                e.to_ascii_lowercase().as_str(),
                "wav" | "mp3" | "flac" | "ogg" | "m4a"
            )
        })
        .unwrap_or(false);
    if !ext_ok {
        return Err(format!("unsupported audio file type: {}", input.display()));
    }

    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let out_dir = std::env::temp_dir().join(format!(
        "livetranslate-tr-{}-{}",
        std::process::id(),
        unique
    ));
    std::fs::create_dir_all(&out_dir).map_err(|e| e.to_string())?;

    let (program, args) = build_command(input, &out_dir);
    let output = Command::new(&program).args(&args).output().map_err(|e| {
        format!("failed to start translator '{program}': {e}. Is the hibiki-zero venv set up?")
    })?;

    if !output.status.success() {
        let _ = std::fs::remove_dir_all(&out_dir);
        return Err(format!(
            "translation failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }

    let produced: Vec<PathBuf> = std::fs::read_dir(&out_dir)
        .map_err(|e| e.to_string())?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .collect();
    let output_wav = match pick_output_wav(&produced) {
        Some(w) => w,
        None => {
            let _ = std::fs::remove_dir_all(&out_dir);
            return Err("translator produced no .wav output".to_string());
        }
    };
    let text = read_translated_text(&produced);

    Ok(TranslationOutput { output_wav, text })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_the_wav_among_other_files() {
        let files = vec![PathBuf::from("log.txt"), PathBuf::from("out_en.wav")];
        assert_eq!(pick_output_wav(&files), Some(PathBuf::from("out_en.wav")));
    }

    #[test]
    fn case_insensitive_extension() {
        let files = vec![PathBuf::from("OUT.WAV")];
        assert_eq!(pick_output_wav(&files), Some(PathBuf::from("OUT.WAV")));
    }

    #[test]
    fn none_when_no_wav() {
        let files = vec![PathBuf::from("a.txt"), PathBuf::from("b.mp3")];
        assert_eq!(pick_output_wav(&files), None);
    }

    #[test]
    fn prefers_stereo_over_mono() {
        let files = vec![
            PathBuf::from("0_clip_mono.wav"),
            PathBuf::from("0_clip_stereo.wav"),
            PathBuf::from("0_clip.txt"),
        ];
        assert_eq!(
            pick_output_wav(&files),
            Some(PathBuf::from("0_clip_stereo.wav"))
        );
    }

    #[test]
    fn falls_back_to_mono_when_no_stereo() {
        let files = vec![PathBuf::from("0_clip_mono.wav")];
        assert_eq!(
            pick_output_wav(&files),
            Some(PathBuf::from("0_clip_mono.wav"))
        );
    }

    #[test]
    fn reads_text_from_txt_file() {
        let dir = std::env::temp_dir().join(format!("lt-txt-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let txt = dir.join("out.txt");
        std::fs::write(&txt, "  Hello world  ").unwrap();
        let files = vec![dir.join("out.wav"), txt.clone()];
        assert_eq!(read_translated_text(&files), "Hello world");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod command_tests {
    use super::*;

    #[test]
    fn build_command_has_generate_file_outdir_bf16() {
        let (_program, args) = build_command(Path::new("in.wav"), Path::new("out"));
        assert_eq!(
            args,
            vec!["generate", "--file", "in.wav", "--out-dir", "out", "--bf16"]
        );
    }

    #[test]
    fn resolve_respects_override() {
        assert_eq!(
            resolve_hibiki_zero_bin(Some("/custom/hibiki-zero".to_string())),
            "/custom/hibiki-zero"
        );
    }

    #[test]
    fn resolve_defaults_to_venv_under_repo_root() {
        let got = resolve_hibiki_zero_bin(None);
        assert!(got.contains(".venv-hibiki"));
        assert!(got.ends_with(if cfg!(windows) {
            "hibiki-zero.exe"
        } else {
            "hibiki-zero"
        }));
    }
}
