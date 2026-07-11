//! Resumable HTTP download core, extracted out of `setup.rs` so both the Python
//! engine setup flow and `models::manager::ModelManager` share one
//! retry/resume implementation instead of duplicating it.
//!
//! This module is progress-reporting-agnostic: callers pass plain closures
//! instead of coupling the download loop to a specific UI (Tauri events,
//! stdout, etc).

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Default number of attempts (initial try + retries) for `download_with_progress`.
pub const DEFAULT_MAX_ATTEMPTS: u32 = 4;

/// Delay before retry number `retry` (1-based): 2s, 4s, 8s, ...
pub fn backoff_delay(retry: u32) -> Duration {
    Duration::from_secs(1u64 << retry)
}

/// In-progress downloads write to `<dest>.part` and are renamed on completion,
/// so `dest` never holds a truncated file.
pub fn part_path(dest: &Path) -> PathBuf {
    let mut name = dest.as_os_str().to_owned();
    name.push(".part");
    PathBuf::from(name)
}

/// A local file needs (re-)downloading when it is missing, or when the remote
/// size is known and differs from the local one. When the remote size cannot
/// be determined the local file is trusted.
pub fn needs_download(local_size: Option<u64>, remote_size: Option<u64>) -> bool {
    match (local_size, remote_size) {
        (None, _) => true,
        (Some(_), None) => false,
        (Some(local), Some(remote)) => local != remote,
    }
}

pub fn http_client() -> Result<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(600))
        .build()
        .map_err(|e| e.to_string())
}

/// Size the server reports for `url`, via a HEAD request. Best-effort.
pub fn remote_content_length(url: &str) -> Option<u64> {
    let client = http_client().ok()?;
    let resp = client.head(url).send().ok()?;
    if !resp.status().is_success() {
        return None;
    }
    resp.content_length().filter(|n| *n > 0)
}

/// Downloads `url` to `dest`, resuming from a `.part` file across retries.
///
/// `on_retry(attempt)` fires just before each retry (`attempt` is the
/// 1-based number of the retry attempt about to run, i.e. 2, 3, ...) so a
/// caller can surface a "retrying…" message. `on_progress(downloaded, total)`
/// fires as bytes arrive; `total` is 0 when the server didn't report a
/// Content-Length.
pub fn download_with_progress(
    url: &str,
    dest: &Path,
    max_attempts: u32,
    on_retry: impl Fn(u32),
    on_progress: impl Fn(u64, u64),
) -> Result<(), String> {
    let part = part_path(dest);
    let mut last_err = String::new();

    for attempt in 1..=max_attempts {
        if attempt > 1 {
            on_retry(attempt);
            std::thread::sleep(backoff_delay(attempt - 1));
        }
        match download_attempt(url, dest, &part, &on_progress) {
            Ok(()) => return Ok(()),
            Err(e) => last_err = e,
        }
    }
    Err(last_err)
}

fn download_attempt(
    url: &str,
    dest: &Path,
    part: &Path,
    on_progress: &impl Fn(u64, u64),
) -> Result<(), String> {
    let client = http_client()?;

    // Resume from an interrupted attempt when a partial file is present.
    let offset = std::fs::metadata(part).map(|m| m.len()).unwrap_or(0);
    let mut req = client.get(url);
    if offset > 0 {
        req = req.header(reqwest::header::RANGE, format!("bytes={offset}-"));
    }

    let mut resp = req.send().map_err(|e| format!("download failed: {e}"))?;
    let status = resp.status();

    if status == reqwest::StatusCode::RANGE_NOT_SATISFIABLE {
        // The partial file is stale or already past the remote size; discard it
        // and let the next attempt start from scratch.
        let _ = std::fs::remove_file(part);
        return Err("server rejected resume range; restarting download".to_string());
    }

    let resumed = offset > 0 && status == reqwest::StatusCode::PARTIAL_CONTENT;
    if !resumed && !status.is_success() {
        return Err(format!("download returned HTTP {status}"));
    }

    let (mut file, mut downloaded, total) = if resumed {
        let file = std::fs::OpenOptions::new()
            .append(true)
            .open(part)
            .map_err(|e| e.to_string())?;
        (file, offset, offset + resp.content_length().unwrap_or(0))
    } else {
        // Fresh download, or the server ignored the Range header: truncate.
        let file = std::fs::File::create(part).map_err(|e| e.to_string())?;
        (file, 0u64, resp.content_length().unwrap_or(0))
    };

    let mut buf = [0u8; 65536];
    loop {
        let n = resp.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n]).map_err(|e| e.to_string())?;
        downloaded += n as u64;
        on_progress(downloaded, total);
    }
    drop(file);

    std::fs::rename(part, dest).map_err(|e| format!("failed to finalize download: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn needs_download_when_local_missing() {
        assert!(needs_download(None, Some(10)));
        assert!(needs_download(None, None));
    }

    #[test]
    fn trusts_local_file_when_remote_size_unknown() {
        assert!(!needs_download(Some(10), None));
    }

    #[test]
    fn redownloads_on_size_mismatch() {
        assert!(needs_download(Some(10), Some(20)));
    }

    #[test]
    fn skips_download_when_sizes_match() {
        assert!(!needs_download(Some(10), Some(10)));
    }

    #[test]
    fn backoff_doubles_per_retry() {
        assert_eq!(backoff_delay(1).as_secs(), 2);
        assert_eq!(backoff_delay(2).as_secs(), 4);
        assert_eq!(backoff_delay(3).as_secs(), 8);
    }

    #[test]
    fn part_path_appends_suffix_without_touching_extension() {
        let p = part_path(Path::new("C:/models/voice.onnx"));
        assert!(p.to_string_lossy().ends_with("voice.onnx.part"));
        let q = part_path(Path::new("C:/models/voice.onnx.json"));
        assert!(q.to_string_lossy().ends_with("voice.onnx.json.part"));
    }
}
