## Exploration: macOS Support via transcribe-rs

### Current State

LiveTranslate currently runs a **Windows-only** pipeline with a Python FastAPI sidecar (`lt_engine`) that handles the full STT → MT → TTS chain:

1. **STT**: NeMo Parakeet TDT 0.6B V3 (`nvidia/parakeet-tdt-0.6b-v3`) loaded via `nemo_toolkit[asr]`
2. **MT**: HuggingFace `Helsinki-NLP/opus-mt-es-en` (MarianMT)
3. **TTS**: Piper TTS (`piper-tts`) with `en_US-lessac-medium.onnx`

The Python server is spawned at startup via `engine_server.rs` → `spawn_server()`, which runs `python -m lt_engine.server`. The Rust side communicates with it via HTTP (`reqwest`). Both online (live) and offline (file) translation flow through this server.

**On macOS**: The Python pipeline technically works (MT and TTS are CPU-friendly), but NeMo has no Apple Silicon wheel — it requires conda or manual builds. `nemo_toolkit[asr]` is painful to install on macOS and the Parakeet model can't leverage Metal/ANE.

### Existing Platform Awareness

The codebase already has platform-conditional code patterns:
- `engine_server.rs` has `#[cfg(target_os = "windows")]` vs `#[cfg(not(target_os = "windows"))]` for port killing
- `sidecar.rs` has `cfg!(windows)` for Python path resolution
- `devices.rs` has tests for BlackHole (macOS) and VB-Cable (Windows)
- Setup installs `nemo_toolkit[asr]` via pip — won't work on macOS without conda

### Handy Reference Architecture

Handy (https://github.com/cjpais/Handy) uses `transcribe-rs` v0.3.8 with:
- **Base features**: `["whisper-cpp", "onnx"]`
- **macOS (`target_os = "macos"`)**: Adds `["whisper-metal"]` for Apple Silicon GPU acceleration
- **Windows**: Adds `["whisper-vulkan", "ort-directml"]`
- Models are downloaded as `.tar.gz` → extracted to `{app_data}/models/`
- Parakeet model loads via `ParakeetModel::load(&path, &Quantization::Int8)`
- Audio is `Vec<f32>` samples at 16 kHz → transcribed to `String`
- The `ort` crate (ONNX Runtime) is pulled in by the `onnx` feature

### Affected Areas

- **`src-tauri/Cargo.toml`** — Add `transcribe-rs` dependency with platform-conditional features
- **`src-tauri/src/translation/engine_server.rs`** — Make server spawning conditional; add Rust-native ASR path for macOS
- **`src-tauri/src/translation/live.rs`** — Worker thread currently calls `engine_server::translate()` for ALL pipeline steps; needs a macOS branching path
- **`src-tauri/src/translation/sidecar.rs`** — `engine_python()` could stay but only for MT+TTS on macOS
- **`src-tauri/src/setup.rs`** — `run_setup_inner()` installs NeMo via pip; needs a macOS path that skips NeMo and downloads Parakeet ONNX models
- **`src-tauri/src/lib.rs`** — Server spawn should be conditional; macOS may not need the Python server if we can do MT+TTS differently
- **`python/lt_engine/pipeline.py`** — On macOS, ASR (transcribe function) becomes unused — but MT+TTS remain
- **`python/lt_engine/server.py`** — Same: `/translate` endpoint would skip ASR step on macOS, receiving pre-transcribed text

### Approaches

#### Approach A: Transcribe-rs for ASR only, keep Python server for MT+TTS (Recommended)

**Description**: Replace only the ASR step on macOS with `transcribe-rs` Parakeet in-process. The Python server still runs but only handles Translation (Opus-MT) + TTS (Piper). The `live.rs` worker splits: on macOS, it calls Rust ASR directly, then sends the transcribed text to the Python server for MT+TTS.

**How it works**:
1. Add `transcribe-rs` to `Cargo.toml` with target-conditional features
2. Add a new module `src-tauri/src/translation/macos_asr.rs` that wraps `ParakeetModel` loading + inference
3. On macOS startup: download Parakeet ONNX model (as tar.gz) instead of running `pip install nemo_toolkit`
4. In `live.rs` `run_worker()`: if macOS, call Rust ASR directly on the WAV bytes, then POST the transcribed text to the Python server for MT+TTS
5. Python server gets a new `/translate_text` endpoint (input: text, output: WAV)
6. The existing `/translate` endpoint (input: audio file) stays for Windows

**Pros**:
- Minimal changes to existing architecture
- MT+TTS already work on macOS (CPU-friendly Python, no CUDA required)
- Fastest path to working macOS support
- Low risk — if transcribe-rs has issues, fall back to full Python
- cpal already works on macOS — audio pipeline unchanged

**Cons**:
- Still requires Python venv on macOS (but smaller — no NeMo)
- Two processes running (Rust ASR + Python MT/TTS server)
- ~456 MB model download for Parakeet V3 int8
- Startup adds ~5-10s for model loading

**Effort**: Medium (2-3 days)

#### Approach B: Full Rust pipeline (transcribe-rs + candle/ort for MT + piper-rs for TTS)

**Description**: Replace ALL Python with Rust. ASR via `transcribe-rs`, MT via exported Opus-MT ONNX model running on `ort` crate, TTS via a Rust Piper binding (or `ort` with the Piper ONNX model).

**How it works**:
1. Same as Approach A for ASR
2. Export `Helsinki-NLP/opus-mt-es-en` to ONNX via `optimum-cli export onnx`
3. Run MT inference with `ort` crate (ONNX Runtime for Rust)
4. Run Piper TTS ONNX model with `ort` crate (Piper models are just ONNX encoder-decoder)
5. No Python at all on macOS

**Pros**:
- Zero Python dependency on macOS
- Single binary, simpler deployment
- Lower memory footprint (no Python runtime)
- Faster startup (no venv activation/server spawning)

**Cons**:
- Exporting Opus-MT to ONNX is non-trivial (MarianMT has custom ops)
- Piper TTS ONNX inference from Rust requires building a small inference engine (tokenizer, audio processing)
- Very high risk — multiple unknowns
- No existing crate does this end-to-end
- The Opus-MT ONNX export may not work cleanly (beam search, custom ops)
- Significantly more code to write and debug

**Effort**: High (2-4 weeks)

#### Approach C: Keep Python server but replace NeMo with transcribe-rs call

**Description**: The Python server still runs but its `transcribe()` function calls a Rust CLI or FFI function instead of NeMo. The Python server orchestrates the full pipeline; it just delegates ASR to the Rust binary.

**How it works**:
1. Build a small Rust CLI tool (`lt_asr`) that loads Parakeet via `transcribe-rs` and outputs JSON
2. Python server calls `subprocess.run(["lt_asr", audio_path])` for transcription
3. Everything else stays the same

**Pros**:
- Minimal changes to Python code
- Single pipeline entry point (Python server)
- Easy to test/fallback

**Cons**:
- Subprocess overhead per transcription (or keep Rust process warm)
- Still need Python + all deps (except NeMo)
- Serialization overhead (WAV file → CLI → JSON)
- Harder to integrate with live streaming (file-based)

**Effort**: Medium (3-4 days)

### Recommendation

**Approach A** — It's the pragmatic choice. Here's why:

1. **MT+TTS work on macOS today**. Opus-MT runs on CPU just fine (it's a small MarianMT model). Piper TTS cross-platform. There's no reason to replace them until there's a clear benefit.
2. **ASR is the bottleneck**. NeMo is the hard part on macOS — no wheel, conda-only, heavy deps. Replacing JUST that with `transcribe-rs` removes the pain point.
3. **Follows Handy's proven path**. Handy uses `transcribe-rs` for Parakeet on macOS exactly as proposed. The API is battle-tested.
4. **Gradual migration**. If later we want to move MT+TTS to Rust (Approach B), we can do it incrementally. Starting with A doesn't block B.
5. **Risk containment**. The existing Windows pipeline is untouched. macOS changes are `#[cfg(target_os = "macos")]` gated. If something goes wrong on macOS, Windows is unaffected.

### Risks

1. **Parakeet ONNX model format**: The `parakeet-tdt-0.6b-v3-int8` model Handy uses is downloaded from Handy's blob storage (`blob.handy.computer`). We need either:
   - Use the same blob (handy's server, could go down)
   - Host our own copy
   - Export from NeMo ourselves (complex, need NeMo export pipeline)
   
2. **ORT linking on Intel Macs**: Requires `brew install onnxruntime` — adds a setup step. Apple Silicon uses static linking automatically.

3. **macOS microphone permissions**: Need `NSMicrophoneUsageDescription` in Info.plist. cpal should handle this but needs testing.

4. **Virtual audio device**: Windows uses VB-Cable. macOS needs BlackHole (or Soundflower). We need to detect and guide installation.

5. **transcribe-rs version compatibility**: The crate evolves fast (v0.1 → v0.3.11 in 6 months). API may break between versions.

6. **Model loading time**: Parakeet V3 int8 is ~456MB. Loading from disk on first use takes 5-10s. Must be done before live translation starts.

7. **Python server still needed**: For MT+TTS, so we still need venv setup on macOS — just a smaller one (no NeMo).

### Ready for Proposal

**Yes** — Approach A has clear scope, known risks, and a well-understood implementation path. Ready for the spec phase.

### Key Technical Answers

| Question | Answer |
|----------|--------|
| transcribe-rs Parakeet V3 support? | Yes — `ParakeetModel::load(&path, &Quantization::Int8)` with the `onnx` feature |
| Model format | Directory-based (extracted from `.tar.gz`): `model.int8.onnx` + config + tokenizer |
| Piper TTS Rust binding? | None well-maintained — must keep Python for TTS |
| Opus-MT → ONNX? | Possible via `optimum` but non-trivial (custom ops in MarianMT) |
| ORT version needed? | Whatever the `ort` crate pulls in (tied to `transcribe-rs` version) |
| Intel Mac ORT install | Yes — `brew install onnxruntime` for dynamic linking |
| macOS permissions needed | Microphone (`NSMicrophoneUsageDescription`), possibly Accessibility for virtual input |
| Handy's dep pattern | Base: `whisper-cpp`, `onnx`. macOS: adds `whisper-metal`. Windows: adds `whisper-vulkan`, `ort-directml` |
