use std::path::{Path, PathBuf};
use std::process::Command;

/// Result of a successful offline translation.
#[derive(Debug, Clone, PartialEq)]
pub struct TranslationOutput {
    pub output_wav: PathBuf,
    pub text: String,
}

/// Picks the translated audio file from the files produced in the work dir.
/// Returns the first `.wav` (case-insensitive) found.
pub fn pick_output_wav(files: &[PathBuf]) -> Option<PathBuf> {
    files
        .iter()
        .find(|p| {
            p.extension()
                .and_then(|e| e.to_str())
                .map(|e| e.eq_ignore_ascii_case("wav"))
                .unwrap_or(false)
        })
        .cloned()
}

/// Path to the hibiki-zero executable. Override with HIBIKI_ZERO_BIN (used by tests
/// and to point at the user's environment); defaults to the project-local venv.
pub fn hibiki_zero_bin() -> String {
    if let Ok(p) = std::env::var("HIBIKI_ZERO_BIN") {
        return p;
    }
    if cfg!(windows) {
        ".venv-hibiki/Scripts/hibiki-zero.exe".to_string()
    } else {
        ".venv-hibiki/bin/hibiki-zero".to_string()
    }
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
    let out_dir = std::env::temp_dir().join(format!("livetranslate-tr-{}", std::process::id()));
    std::fs::create_dir_all(&out_dir).map_err(|e| e.to_string())?;

    let (program, args) = build_command(input, &out_dir);
    let output = Command::new(&program).args(&args).output().map_err(|e| {
        format!("failed to start translator '{program}': {e}. Is the hibiki-zero venv set up?")
    })?;

    if !output.status.success() {
        return Err(format!(
            "translation failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }

    let produced: Vec<PathBuf> = std::fs::read_dir(&out_dir)
        .map_err(|e| e.to_string())?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .collect();
    let output_wav = pick_output_wav(&produced).ok_or("translator produced no .wav output")?;
    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();

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
    fn hibiki_zero_bin_respects_env_override() {
        std::env::set_var("HIBIKI_ZERO_BIN", "/custom/hibiki-zero");
        assert_eq!(hibiki_zero_bin(), "/custom/hibiki-zero");
        std::env::remove_var("HIBIKI_ZERO_BIN");
    }
}
