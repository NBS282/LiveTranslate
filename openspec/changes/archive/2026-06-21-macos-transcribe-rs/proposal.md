# Proposal: macOS Support via transcribe-rs

## Intent
macOS users cannot run LiveTranslate — NeMo has no Apple Silicon wheel. MT+TTS already work on macOS CPU. Replace only ASR with `transcribe-rs` in-process; keep Python for MT+TTS.

## Scope

### In Scope
- `transcribe-rs` dep with `whisper-metal` (macOS) / `whisper-cpp`+`onnx` (base)
- `macos_asr.rs`: Parakeet model load + transcribe wrapper
- Platform-conditional server spawn (full Python on Windows, MT+TTS-only on macOS)
- `/translate_text` endpoint on Python server (text → WAV)
- Parakeet ONNX model download on setup (macOS only)
- `NSMicrophoneUsageDescription` in Info.plist
- BlackHole setup guidance for virtual audio

### Out of Scope
- Full Rust pipeline (MT+TTS) — deferred
- Intel Mac ORT auto-setup — documented, not automated
- Model hosting infra — uses existing blob or manual download
- Windows pipeline — untouched

## Capabilities

### New Capabilities
- `macos-asr`: On-device STT via Parakeet V3 ONNX in Rust with Metal GPU on Apple Silicon

### Modified Capabilities
- `live-translate`: ASR path branches by platform — Python NeMo (Windows) vs Rust `transcribe-rs` (macOS); `/translate_text` added for text→WAV

## Approach
**transcribe-rs for ASR only, Python for MT+TTS**:

1. `Cargo.toml`: `transcribe-rs = "0.3"` with base features + `whisper-metal` gated to `cfg(target_os = "macos")`
2. New `macos_asr.rs`: wraps `ParakeetModel::load()` + inference
3. `setup.rs`: macOS downloads Parakeet `.tar.gz` → app data dir
4. `engine_server.rs`: macOS spawns Python with MT+TTS only (no NeMo)
5. `server.py`: new `/translate_text` — `{"text": "..."}` in → WAV out
6. `live.rs`: on macOS, Rust ASR → POST text to `/translate_text`

## Affected Areas

| Area | Impact | Description |
|------|--------|-------------|
| `Cargo.toml` | Modified | Add `transcribe-rs` with platform features |
| `translation/macos_asr.rs` | New | Parakeet model wrapper (macOS) |
| `translation/live.rs` | Modified | macOS ASR → text → POST path |
| `translation/engine_server.rs` | Modified | Conditional spawn, text endpoint |
| `translation/sidecar.rs` | Modified | macOS Python path |
| `setup.rs` | Modified | macOS model download |
| `lib.rs` | Modified | Conditional server spawn |
| `python/lt_engine/server.py` | Modified | `/translate_text` endpoint |
| `tauri.conf.json` | Modified | `NSMicrophoneUsageDescription` |

## Risks

| Risk | Likelihood | Mitigation |
|------|------------|------------|
| Parakeet model hosting unavailable | Med | Fallback: documented manual download |
| Intel Mac ORT linking | Med | Document `brew install onnxruntime` |
| transcribe-rs API breakage | Low | Pin version, test before upgrade |
| Model load time 5-10s | Low | Load at startup, block live until ready |

## Rollback Plan
Revert `Cargo.toml`, `macos_asr.rs`, changes to `live.rs`/`engine_server.rs`. macOS returns to unsupported state (status quo ante).

## Dependencies
- `transcribe-rs` 0.3.x (`whisper-cpp`, `onnx`, `whisper-metal`)
- Parakeet V3 int8 ONNX model
- `cpal` 0.15 (already in tree)

## Success Criteria
- [ ] macOS live Spanish→EN translation works end-to-end
- [ ] Transcription latency < 2× Windows baseline
- [ ] Python server starts on macOS without NeMo error
- [ ] All existing Windows tests pass
