# Tasks: macOS Support via transcribe-rs

## Review Workload Forecast

| Field | Value |
|-------|-------|
| Estimated changed lines | ~350-400 |
| 400-line budget risk | Medium |
| Chained PRs recommended | No |
| Suggested split | Single PR |
| Delivery strategy | single-pr |
| Chain strategy | none (single PR) |

Decision needed before apply: No
Chained PRs recommended: No
Chain strategy: pending
400-line budget risk: Medium

### Suggested Work Units

| Unit | Goal | Likely PR | Notes |
|------|------|-----------|-------|
| 1 | Python + Foundation | PR 1 | `/translate_text` endpoint, Cargo deps, `macos_asr.rs` base, Parakeet download. Base: `main` |
| 2 | Integration + Tests | PR 2 | `live.rs` split, engine_server integration, lib.rs, setup wiring, all tests. Depends on PR 1 |

## Phase 1: Python Server Changes

- [x] 1.1 `pipeline.py` — Add `translate_text()` calling `translate()`+`synthesize()`. Condition `warmup()` to skip `_get_asr()` when `LT_SKIP_ASR` is set
- [x] 1.2 `server.py` — Add `TranslateTextRequest` model + POST `/translate_text` handler

## Phase 2: Rust Foundation

- [x] 2.1 `Cargo.toml` — Add `transcribe-rs` with `whisper-cpp`+`onnx` base + `[target.'cfg(target_os = "macos")'.dependencies]` for `whisper-metal`
- [x] 2.2 `translation/macos_asr.rs` (NEW) — `init_model()` singleton wrapping `ParakeetModel::load()` + `transcribe_segment(&[i16]) -> Result<String, String>` with thread-safe `OnceLock`
- [x] 2.3 `translation/mod.rs` — `#[cfg(target_os = "macos")] pub mod macos_asr;`

## Phase 3: Rust Integration

- [x] 3.1 `engine_server.rs` — Add `translate_text(&str) -> Result<TranslationOutput, String>` + `parse_translate_text_response()`. Set `LT_SKIP_ASR=1` in `spawn_server()` on macOS
- [x] 3.2 `live.rs` — `#[cfg(target_os = "macos")]` run_worker: `macos_asr::transcribe_segment()` → POST `/translate_text`. Windows path: unchanged via `#[cfg(not(target_os = "macos"))]`
- [x] 3.3 `lib.rs` — macOS spawns server without NeMo wait. Rename `check_vbcable` → `check_virtual_audio` using `find_virtual_output()` with `["blackhole", "vb-audio", "cable"]`

## Phase 4: Setup & Config

- [x] 4.1 `setup.rs` — Add Parakeet ONNX download + extract to `models/parakeet/` (macOS only via `#[cfg]`). Include in `check()` status
- [x] 4.2 `tauri.conf.json` — Add `NSMicrophoneUsageDescription` under `app.security`

## Phase 5: Testing

- [x] 5.1 `python/tests/test_server.py` — Test `/translate_text`: valid text → WAV+text, empty text → 422, missing model returns error
- [x] 5.2 Unit tests for `macos_asr` — Error on missing model file, error wrapping matches `Result<String, String>` signature (behind `#[cfg(target_os = "macos")]`)
- [x] 5.3 `engine_server.rs` — Add `parse_translate_text_response()` JSON parsing tests
- [x] 5.4 Windows regression — All existing `cargo test` + Python tests pass unchanged (28 Rust + 7 Python)
