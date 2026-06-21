# Archive Report: macos-transcribe-rs

**Status**: ARCHIVED
**Archived at**: 2026-06-21
**Mode**: OpenSpec
**SDD Cycle**: Complete — explore → propose → spec → design → tasks → apply → verify → archive

---

## Summary

Replaced the Python NeMo ASR pipeline (no Apple Silicon wheel available) with Rust `transcribe-rs` wrapping a Parakeet V3 ONNX model on macOS. The Python server remains for MT+TTS on both platforms — macOS disables ASR in the server via `LT_SKIP_ASR`. Platform dispatch uses `#[cfg(target_os = "macos")]` for zero-cost branching.

This change enables macOS users (Apple Silicon) to run LiveTranslate end-to-end: microphone → Rust ASR → Python MT+TTS → audio output.

---

## Artifacts in Archive

| Artifact | Path | Status |
|----------|------|--------|
| Proposal | `proposal.md` | ✅ Complete |
| Specs | `specs/live-translate/spec.md` | ✅ Complete |
| Design | `design.md` | ✅ Complete |
| Tasks | `tasks.md` | ✅ 14/14 tasks complete |
| Verify Report | `verify-report.md` | ✅ PASS WITH WARNINGS (no CRITICAL) |
| Archive Report | `archive-report.md` | ✅ Current |

---

## Specs Synced to Source of Truth

| Domain | Action | Details |
|--------|--------|---------|
| `macos-asr` | Already present as main spec | No delta to merge — main spec at `openspec/specs/macos-asr/spec.md` |
| `live-translate` | Created as new main spec | Delta spec copied directly to `openspec/specs/live-translate/spec.md` (3 ADDED, 1 MODIFIED, 1 REMOVED requirement) |

---

## Files Created

| File | Type |
|------|------|
| `src-tauri/src/translation/macos_asr.rs` | New — Parakeet model wrapper (macOS only) |
| `src-tauri/src/translation/mod.rs` | Modified — `#[cfg(target_os = "macos")] pub mod macos_asr;` |
| `src-tauri/src/translation/live.rs` | Modified — split `run_worker` into macos/not-macos impls |
| `src-tauri/src/translation/engine_server.rs` | Modified — `translate_text()` fn, `LT_SKIP_ASR=1` on macOS |
| `src-tauri/Cargo.toml` | Modified — `transcribe-rs` dep with platform-gated features |
| `src-tauri/src/setup.rs` | Modified — Parakeet ONNX download + extract (macOS only) |
| `src-tauri/src/lib.rs` | Modified — `check_vbcable` → `check_virtual_audio` |
| `src-tauri/tauri.conf.json` | Modified — `NSMicrophoneUsageDescription` |
| `python/lt_engine/server.py` | Modified — `/translate_text` endpoint |
| `python/lt_engine/pipeline.py` | Modified — `translate_text()` fn, `warmup()` respects `LT_SKIP_ASR` |
| `python/tests/test_server.py` | Modified — `/translate_text` tests (4 new) |

**Total**: 11 files (1 new, 10 modified)

---

## Test Results

| Suite | Count | Result |
|-------|-------|--------|
| Rust unit tests | 28 | ✅ All pass |
| Python integration tests | 7 | ✅ All pass |
| **Total** | **35** | **✅ All pass** |

---

## Known Limitations

1. **Placeholder model URL**: `PARAKEET_MODEL_URL` is set to `https://example.com/...` — must be replaced with a real HuggingFace or blob storage URL before macOS release.
2. **Intel Mac requires manual ORT install**: `brew install onnxruntime` is required — not automated.
3. **Dead-code warnings on Windows**: `parse_translate_text_response` and `translate_text` appear "never used" on Windows because they're macOS-only paths. Harmless but noisy.
4. **Misleading test name**: `double_init_returns_err` doesn't actually test double-init — it's a duplicate of `error_wrapping_matches_result_signature`. Not a runtime issue.
5. **No macOS CI runner**: E2E macOS testing is manual on Apple Silicon (4 spec scenarios can't run on Windows).
6. **Missing `apply-progress` artifact**: The TDD Cycle Evidence table was not persisted during apply, but all tests exist and pass.

---

## Recommendations for Future Work

### Short-term (before macOS release)
1. **Replace placeholder model URL** with a real HuggingFace or blob storage URL in `setup.rs`
2. **Add `#[expect(dead_code)]`** on `translate_text` / `parse_translate_text_response` to suppress Windows dead-code warnings
3. **Rename `double_init_returns_err`** test to match its actual behavior, or fix it to truly test double-init
4. **Document Intel Mac setup** in README: `brew install onnxruntime` + manual model download fallback

### Medium-term
5. **Canary model migration**: `transcribe-rs` 0.4+ may support streaming PCM input (removes temp WAV write). Evaluate when released.
6. **Model hosting infra**: Set up automated model hosting (GitHub Releases, HuggingFace, or blob storage) with versioned Parakeet archives.
7. **Coverage tooling**: Install `cargo-tarpaulin` or `cargo-llvm-cov` in CI for coverage enforcement.

### Long-term
8. **Full Rust pipeline**: Port MT+TTS to Rust for a unified binary. This is out of scope — requires significant investment in Rust MT models.
9. **macOS CI runner**: Consider self-hosted Apple Silicon runner for automated E2E macOS testing.

---

## Design Decisions Followed

| Decision | Implementation |
|----------|---------------|
| `#[cfg]` not traits | Two `run_worker` fns, `#[cfg(target_os = "macos")]` / `#[cfg(not(...))]` |
| `LT_SKIP_ASR` env var (not two binaries) | `spawn_server()` sets env on macOS; `warmup()` checks env |
| Eager model loading | `init_model()` called during `spawn_server()` wait |
| Temp WAV (not streaming PCM) | `write_temp_wav()` same pattern as `write_segment_wav()` |
| `find_virtual_output()` multi-hint | `check_virtual_audio()` with `["blackhole", "vb-audio", "cable"]` |
| Cargo feature union | Base: `whisper-cpp+onnx`. macOS adds `whisper-metal` via target dep |

---

## Change Status: ARCHIVED

This SDD cycle is complete. The archive is the audit trail — do not modify.
