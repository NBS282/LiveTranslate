# Design: macOS Support via transcribe-rs

## Technical Approach

Replace NeMo ASR (no Apple Silicon wheel) with `transcribe-rs` (Rust, Parakeet V3 ONNX, Metal GPU) on macOS. The Python server is unconditional (needed for MT+TTS on both platforms) but skips NeMo loading on macOS via `LT_SKIP_ASR` env var. The live pipeline's ASR step branches at compile time: `#[cfg(target_os = "macos")]` calls `macos_asr::transcribe_segment()` in-process; Windows writes a temp WAV and POSTs to Python.

## Architecture Decisions

| Option | Tradeoffs | Decision |
|--------|-----------|----------|
| Platform dispatch | Traits (flexible, boilerplate) vs `#[cfg]` (zero-cost, local) | `#[cfg]` — two `run_worker` fns, same signature, caller agnostic |
| Server spawn | Two binaries (duplication) vs env-var gating (one binary) | `LT_SKIP_ASR` env var in `spawn_server()` -> Python `warmup()` skips `_get_asr()` |
| Model load timing | Lazy (first-segment latency spike) vs eager at startup (consistent) | Eager, same as server warmup — `init_model()` called during `spawn_server()` wait |
| transcribe-rs API | Stream PCM (unsupported in 0.3) vs temp WAV (documented) | Temp WAV — same pattern as existing `write_segment_wav()` |
| Virtual audio check | Hardcoded "cable" (misses BlackHole) vs `find_virtual_output()` (multi-hint) | Reuse existing `find_virtual_output()` — already handles "blackhole", "vb-audio", "cable" |

## Data Flow

```
macOS live path:

Mic → cpal → resample → VAD → Segmenter → macos_asr::transcribe_segment()
                                                 ↓ (text)
                                              POST /translate_text → MT+TTS → WAV → play_wav_to_device()

Windows live path (unchanged):

Mic → cpal → resample → VAD → Segmenter → write_segment_wav()
                                                 ↓ (WAV file)
                                              POST /translate → ASR+MT+TTS → WAV → play_wav_to_device()
```

## File Changes

| File | Action | Description |
|------|--------|-------------|
| `src-tauri/Cargo.toml` | Modify | Add `transcribe-rs` 0.3 with `base` dep + platform-gated `whisper-metal` target dep |
| `src-tauri/src/translation/macos_asr.rs` | Create | `ParakeetModel` singleton wrapper: `init_model()`, `transcribe_segment()` |
| `src-tauri/src/translation/mod.rs` | Modify | `#[cfg(target_os = "macos")] pub mod macos_asr;` |
| `src-tauri/src/translation/live.rs` | Modify | Split `run_worker` into macos/not-macos impls; Windows unchanged |
| `src-tauri/src/translation/engine_server.rs` | Modify | Add `translate_text()` fn; set `LT_SKIP_ASR=1` in `spawn_server()` env on macOS |
| `src-tauri/src/setup.rs` | Modify | macOS: download+extract Parakeet ONNX `.tar.gz` to `models/parakeet/` |
| `src-tauri/src/lib.rs` | Modify | `check_vbcable` → `check_virtual_audio` using `find_virtual_output()` |
| `src-tauri/tauri.conf.json` | Modify | Add `NSMicrophoneUsageDescription` under `app.security` for macOS |
| `python/lt_engine/server.py` | Modify | Add `/translate_text` endpoint, `TranslateTextRequest` model |
| `python/lt_engine/pipeline.py` | Modify | Add `translate_text()` fn, `warmup()` respects `LT_SKIP_ASR` |

## Interfaces / Contracts

### `translation::macos_asr` (macOS only)

```rust
/// Loads Parakeet model from `models_dir/parakeet/`. Call once at startup.
pub fn init_model(models_dir: &Path) -> Result<(), String>

/// Transcribes 16 kHz mono i16 audio samples to text.
pub fn transcribe_segment(samples: &[i16]) -> Result<String, String>
```

### `engine_server.rs` additions

```rust
/// POSTs transcribed text to /translate_text, returns TranslationOutput
pub fn translate_text(text: &str) -> Result<TranslationOutput, String>
```

### Python `/translate_text` endpoint

```python
# POST /translate_text
# Request: {"text": "string", "out_dir": "string", "src": "es", "tgt": "en"}
# Response: {"output_wav": "string", "source_text": "string", "translated_text": "string"}
# Skips transcribe(), calls translate() + synthesize()
def translate_text(text: str, out_dir: str, src: str, tgt: str) -> dict
```

### Cargo dependency layout

```toml
[dependencies]
transcribe-rs = { version = "0.3", default-features = false, features = ["whisper-cpp", "onnx"] }

[target.'cfg(target_os = "macos")'.dependencies]
transcribe-rs = { version = "0.3", features = ["whisper-metal"] }
```

Cargo unions features across target-specific and base deps. macOS gets `whisper-cpp + onnx + whisper-metal`. Windows/others get `whisper-cpp + onnx`. No feature flags needed in our Cargo.toml.

## Testing Strategy

| Layer | What | Approach |
|-------|------|----------|
| Unit | `engine_server::translate_text()` response parsing | Mocked HTTP + `parse_translate_response` (reuses existing pattern) |
| Unit | Virtual audio detection | `devices.rs` already has BlackHole tests — update `check_virtual_audio` |
| Integration | Python `/translate_text` endpoint | `TestClient` same pattern as `test_server.py` — monkeypatch `translate_text()`, verify response |
| E2E | macOS full live path | Manual on Apple Silicon (no GH macOS runner currently). Verify: speech→ASR→MT→TTS→playback |
| Regression | Windows pipeline unchanged | All existing `cargo test` + Python tests pass |

## Migration / Rollout

No migration required. The Windows path is completely unchanged — `#[cfg(not(target_os = "macos"))]` preserves the existing `run_worker` verbatim. macOS users get the new path automatically when building on Apple Silicon.

**Risks requiring documentation**:
- Intel Mac users: need `brew install onnxruntime` before Parakeet model will load (ORT not bundled)
- Manual model download fallback: if Parakeet archive URL changes, document download-and-extract-to `models/parakeet/`

## Open Questions

- [ ] transcribe-rs `ParakeetModel` API surface — confirm method signature before implementation (0.3 API not yet verified against actual crate source)
- [ ] Parakeet V3 int8 ONNX model URL — needs to be sourced (HuggingFace or blob storage)
- [ ] `whisper-metal` feature compatibility with `whisper-cpp` and `onnx` features in the same build — verify no linker conflicts
