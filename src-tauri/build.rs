use std::fs;

fn main() {
    embed_hf_token();
    tauri_build::build()
}

/// Embeds the Hugging Face token into the binary at compile time so the packaged
/// app can download the gated Pocket TTS voice-cloning weights without shipping
/// the token in source.
///
/// Resolution order: an existing `HF_TOKEN` build-environment variable (CI/shell)
/// wins; otherwise the token is read from `.env.local` at the repo root, which is
/// git-ignored. If neither is present the token is left unset and the code falls
/// back to an empty string (voice cloning unavailable, but the build still works).
fn embed_hf_token() {
    // Rebuild when the local secret or the env var changes so the embedded value
    // never goes stale.
    println!("cargo:rerun-if-changed=../.env.local");
    println!("cargo:rerun-if-env-changed=HF_TOKEN");

    let token = std::env::var("HF_TOKEN")
        .ok()
        .filter(|v| !v.is_empty())
        .or_else(read_token_from_env_local);

    if let Some(token) = token {
        println!("cargo:rustc-env=HF_TOKEN={token}");
    }
}

/// Parses `HF_TOKEN=...` out of `.env.local` (repo root, one level up from src-tauri).
fn read_token_from_env_local() -> Option<String> {
    let contents = fs::read_to_string("../.env.local").ok()?;
    for line in contents.lines() {
        let line = line.trim();
        if let Some(value) = line.strip_prefix("HF_TOKEN=") {
            let value = value
                .trim()
                .trim_matches('"')
                .trim_matches('\'')
                .to_string();
            if !value.is_empty() {
                return Some(value);
            }
        }
    }
    None
}
