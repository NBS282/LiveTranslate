# Streaming Sub-phase A — Persistent Translation Server Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the per-file engine invocation with a persistent FastAPI server (`lt_engine.server`) that loads the models once and translates over HTTP, launched and managed by the Tauri app, so steady-state translation drops from ~20s (reload) to ~2s.

**Architecture:** A FastAPI server holds Parakeet/NLLB/Piper as module singletons, exposes `GET /health` and `POST /translate`. The Rust app spawns it as a sidecar on `127.0.0.1:8765`, polls `/health` until ready, and `translate_file` now POSTs to it (instead of spawning a process per call). The server child is killed on app exit.

**Tech Stack:** Python `fastapi` + `uvicorn` (added to the engine venv), existing `lt_engine` pipeline; Rust `reqwest` (blocking) HTTP client; Tauri 2.

## Global Constraints

- **CPU-only.** No GPU, no CUDA, stable torch CPU. (unchanged from the modular engine)
- **Cross-platform:** Windows, macOS, Linux.
- Server binds **`127.0.0.1` only** (never `0.0.0.0`). No auth (local IPC). Port via env `LT_ENGINE_PORT`, default `8765`.
- Engine venv `.venv-engine` (gitignored). Engine python resolved via `LT_ENGINE_PYTHON` env or repo-root default (existing helper).
- Artifacts in English. `translate_file(&Path) -> Result<TranslationOutput, String>` signature is preserved.

---

### Task 1: Python — model singletons + FastAPI server

**Files:**
- Modify: `python/lt_engine/pipeline.py`
- Create: `python/lt_engine/server.py`
- Create: `python/tests/test_server.py`
- Modify: engine venv deps (install `fastapi`, `uvicorn`)

**Interfaces:**
- Produces: `lt_engine.pipeline.translate_audio(input_path, out_dir, src, tgt) -> dict` returning `{"output_wav": str, "source_text": str, "translated_text": str}`; and the server endpoints `GET /health -> {"ready": bool}`, `POST /translate {input_path,out_dir,src?,tgt?} -> {output_wav,source_text,translated_text}` consumed by Task 2.

- [ ] **Step 1: Install server deps in the engine venv**

Run: `.venv-engine/Scripts/python -m pip install fastapi "uvicorn[standard]"` (or `uv pip install fastapi "uvicorn[standard]"` with the venv active).

- [ ] **Step 2: Refactor `pipeline.py` to load models once (lazy singletons) + add a single `translate_audio`**

In `python/lt_engine/pipeline.py`, replace the per-call model creation in `transcribe`/`translate`/`synthesize` with module-level lazy singletons, and add an orchestration function `translate_audio`. Keep `normalize_lang` unchanged.
```python
import os, json, wave

_asr = None
_nllb = None        # tuple (tokenizer, model)
_piper = None

def _get_asr():
    global _asr
    if _asr is None:
        from nemo.collections.asr.models import ASRModel
        _asr = ASRModel.from_pretrained("nvidia/parakeet-tdt-0.6b-v3", map_location="cpu")
    return _asr

def _get_nllb():
    global _nllb
    if _nllb is None:
        from transformers import AutoTokenizer, AutoModelForSeq2SeqLM
        name = "facebook/nllb-200-distilled-600M"
        _nllb = (AutoTokenizer.from_pretrained(name), AutoModelForSeq2SeqLM.from_pretrained(name))
    return _nllb

def _get_piper():
    global _piper
    if _piper is None:
        from piper import PiperVoice
        _piper = PiperVoice.load(_piper_voice_path())
    return _piper

def transcribe(audio_path: str) -> str:
    out = _get_asr().transcribe([audio_path])
    item = out[0]
    return getattr(item, "text", item)

def translate(text: str, src: str, tgt: str) -> str:
    tok, model = _get_nllb()
    tok.src_lang = src
    inputs = tok(text, return_tensors="pt")
    gen = model.generate(**inputs, forced_bos_token_id=tok.convert_tokens_to_ids(tgt), max_length=512)
    return tok.batch_decode(gen, skip_special_tokens=True)[0]

def synthesize(text: str, out_wav: str) -> None:
    voice = _get_piper()
    with wave.open(out_wav, "wb") as wf:
        voice.synthesize_wav(text, wf)

def warmup() -> None:
    """Force all models to load (called at server startup)."""
    _get_asr(); _get_nllb(); _get_piper()

def translate_audio(input_path: str, out_dir: str, src: str = "es", tgt: str = "en") -> dict:
    os.makedirs(out_dir, exist_ok=True)
    s, t = normalize_lang(src), normalize_lang(tgt)
    source_text = transcribe(input_path)
    if not source_text.strip():
        raise ValueError("transcription produced no text")
    translated_text = translate(source_text, s, t)
    out_wav = os.path.join(out_dir, "output.wav")
    synthesize(translated_text, out_wav)
    return {"output_wav": out_wav, "source_text": source_text, "translated_text": translated_text}
```
Keep `_piper_voice_path` and `normalize_lang` as they are. Update `__main__.py` to call `translate_audio` (replace its inline STT→MT→TTS body with `result = translate_audio(args.file, args.out_dir, args.src, args.tgt)` then write `result.json` from that dict; the empty-transcript guard now lives in `translate_audio` and raises — catch it in `__main__` and return 1).

- [ ] **Step 3: Create the FastAPI server**

`python/lt_engine/server.py`:
```python
"""Persistent FastAPI translation server. Loads models once at startup. Binds 127.0.0.1 only."""
import os
from contextlib import asynccontextmanager
from fastapi import FastAPI, HTTPException
from pydantic import BaseModel
from .pipeline import translate_audio, warmup

@asynccontextmanager
async def lifespan(app: FastAPI):
    warmup()          # load all models before serving requests
    yield

app = FastAPI(lifespan=lifespan)

class TranslateRequest(BaseModel):
    input_path: str
    out_dir: str
    src: str = "es"
    tgt: str = "en"

@app.get("/health")
def health() -> dict:
    return {"ready": True}

@app.post("/translate")
def do_translate(req: TranslateRequest) -> dict:
    if not os.path.isfile(req.input_path):
        raise HTTPException(status_code=400, detail=f"input not found: {req.input_path}")
    try:
        return translate_audio(req.input_path, req.out_dir, req.src, req.tgt)
    except ValueError as e:
        raise HTTPException(status_code=422, detail=str(e))
    except Exception as e:  # noqa: BLE001 - surface engine errors to the client
        raise HTTPException(status_code=500, detail=f"translation failed: {e}")

def main() -> None:
    import uvicorn
    port = int(os.environ.get("LT_ENGINE_PORT", "8765"))
    uvicorn.run(app, host="127.0.0.1", port=port, log_level="info")

if __name__ == "__main__":
    main()
```
Because `lifespan` runs `warmup()` before uvicorn accepts requests, a successful `GET /health` (200) implies models are loaded.

- [ ] **Step 4: Test the server contract with mocked models (TestClient)**

`python/tests/test_server.py`:
```python
from fastapi.testclient import TestClient
import lt_engine.server as server

def test_health_ok(monkeypatch):
    # avoid loading real models during lifespan
    monkeypatch.setattr(server, "warmup", lambda: None)
    with TestClient(server.app) as client:
        r = client.get("/health")
        assert r.status_code == 200
        assert r.json() == {"ready": True}

def test_translate_calls_engine(monkeypatch, tmp_path):
    monkeypatch.setattr(server, "warmup", lambda: None)
    captured = {}
    def fake(input_path, out_dir, src, tgt):
        captured.update(input_path=input_path, src=src, tgt=tgt)
        return {"output_wav": "out.wav", "source_text": "hola", "translated_text": "hello"}
    monkeypatch.setattr(server, "translate_audio", fake)
    f = tmp_path / "in.wav"; f.write_bytes(b"x")
    with TestClient(server.app) as client:
        r = client.post("/translate", json={"input_path": str(f), "out_dir": str(tmp_path)})
        assert r.status_code == 200
        assert r.json()["translated_text"] == "hello"
        assert captured["src"] == "es"

def test_translate_missing_file_400(monkeypatch):
    monkeypatch.setattr(server, "warmup", lambda: None)
    with TestClient(server.app) as client:
        r = client.post("/translate", json={"input_path": "/no/such.wav", "out_dir": "."})
        assert r.status_code == 400
```
Note: `do_translate` references the module-level `translate_audio`; the test monkeypatches `server.translate_audio`, so import it as `from .pipeline import translate_audio` (already done) — monkeypatching `server.translate_audio` rebinds the name the endpoint uses.

- [ ] **Step 5: Run tests**

Run: `cd python && ../.venv-engine/Scripts/python -m pytest tests/ -v`
Expected: existing langcode tests + 3 new server tests pass.

- [ ] **Step 6: Commit**

```bash
git add python/lt_engine/pipeline.py python/lt_engine/server.py python/lt_engine/__main__.py python/tests/test_server.py
git commit -m "feat(engine): persistent FastAPI server with model singletons"
```

---

### Task 2: Rust — spawn the server, health-check, and route translate_file over HTTP

**Files:**
- Create: `src-tauri/src/translation/engine_server.rs`
- Modify: `src-tauri/src/translation/mod.rs` (add `pub mod engine_server;`)
- Modify: `src-tauri/src/translation/sidecar.rs` (translate_file → HTTP)
- Modify: `src-tauri/src/state.rs` (hold server child handle)
- Modify: `src-tauri/src/lib.rs` (spawn server on setup, manage shutdown)
- Modify: `src-tauri/Cargo.toml` (add `reqwest`)

**Interfaces:**
- Consumes: server endpoints from Task 1 (`/health`, `/translate`).
- Produces: `engine_server::ensure_started() -> Result<(), String>`, `engine_server::translate(input: &Path) -> Result<TranslationOutput, String>`, `engine_server::base_url() -> String`. `translate_file` delegates to `engine_server::translate`.

- [ ] **Step 1: Add reqwest**

In `src-tauri/Cargo.toml` `[dependencies]`: `reqwest = { version = "0.12", features = ["blocking", "json"] }`.

- [ ] **Step 2: TDD the pure response parsing**

Create `src-tauri/src/translation/engine_server.rs`:
```rust
use std::path::{Path, PathBuf};
use crate::translation::sidecar::TranslationOutput;

fn port() -> String {
    std::env::var("LT_ENGINE_PORT").unwrap_or_else(|_| "8765".to_string())
}
pub fn base_url() -> String {
    format!("http://127.0.0.1:{}", port())
}

/// Parses the /translate JSON response into a TranslationOutput.
pub fn parse_translate_response(body: &str) -> Result<TranslationOutput, String> {
    let v: serde_json::Value = serde_json::from_str(body).map_err(|e| e.to_string())?;
    let wav = v.get("output_wav").and_then(|x| x.as_str())
        .ok_or("response missing output_wav")?;
    let src = v.get("source_text").and_then(|x| x.as_str()).unwrap_or("").to_string();
    let tgt = v.get("translated_text").and_then(|x| x.as_str()).unwrap_or("").to_string();
    Ok(TranslationOutput { output_wav: PathBuf::from(wav), source_text: src, translated_text: tgt })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_full_response() {
        let out = parse_translate_response(
            r#"{"output_wav":"C:/t/output.wav","source_text":"hola","translated_text":"hello"}"#
        ).unwrap();
        assert_eq!(out.output_wav, PathBuf::from("C:/t/output.wav"));
        assert_eq!(out.translated_text, "hello");
    }
    #[test]
    fn missing_output_wav_errors() {
        assert!(parse_translate_response(r#"{"source_text":"x"}"#).is_err());
    }
    #[test]
    fn invalid_json_errors() {
        assert!(parse_translate_response("nope").is_err());
    }
}
```
Add `pub mod engine_server;` to `src-tauri/src/translation/mod.rs`. Run: `cd src-tauri && cargo test engine_server` (RED→GREEN).

- [ ] **Step 3: Implement spawn + health-check + translate**

Append to `engine_server.rs`:
```rust
use std::process::{Child, Command};
use std::time::{Duration, Instant};

/// Spawns the FastAPI server as a child process (engine venv python -m lt_engine.server).
pub fn spawn_server() -> Result<Child, String> {
    let program = crate::translation::sidecar::engine_python();
    let cwd = crate::translation::sidecar::python_dir();
    Command::new(&program)
        .args(["-m", "lt_engine.server"])
        .current_dir(cwd)
        .spawn()
        .map_err(|e| format!("failed to spawn translation server '{program}': {e}"))
}

/// Polls /health until ready or the timeout elapses.
pub fn wait_until_ready(timeout: Duration) -> Result<(), String> {
    let client = reqwest::blocking::Client::new();
    let url = format!("{}/health", base_url());
    let start = Instant::now();
    while start.elapsed() < timeout {
        if let Ok(resp) = client.get(&url).timeout(Duration::from_secs(2)).send() {
            if resp.status().is_success() {
                return Ok(());
            }
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    Err("translation server did not become ready in time".to_string())
}

/// Sends an audio file to the server for translation.
pub fn translate(input: &Path) -> Result<TranslationOutput, String> {
    if !input.exists() {
        return Err(format!("input file not found: {}", input.display()));
    }
    let ext_ok = input.extension().and_then(|e| e.to_str())
        .map(|e| matches!(e.to_ascii_lowercase().as_str(), "wav" | "mp3" | "flac" | "ogg" | "m4a"))
        .unwrap_or(false);
    if !ext_ok {
        return Err(format!("unsupported audio file type: {}", input.display()));
    }
    let unique = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos()).unwrap_or(0);
    let out_dir = std::env::temp_dir().join(format!("livetranslate-tr-{}-{}", std::process::id(), unique));
    std::fs::create_dir_all(&out_dir).map_err(|e| e.to_string())?;

    let client = reqwest::blocking::Client::new();
    let resp = client.post(format!("{}/translate", base_url()))
        .json(&serde_json::json!({
            "input_path": input.to_string_lossy(),
            "out_dir": out_dir.to_string_lossy(),
            "src": "es", "tgt": "en"
        }))
        .timeout(Duration::from_secs(120))
        .send()
        .map_err(|e| format!("translation request failed: {e}"))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().unwrap_or_default();
        return Err(format!("translation server error {status}: {body}"));
    }
    let body = resp.text().map_err(|e| e.to_string())?;
    parse_translate_response(&body)
}
```
Make `engine_python` and `python_dir` in `sidecar.rs` `pub` (they're currently private helpers) so `engine_server` can reuse them. Keep `TranslationOutput` and `parse_result_json` in `sidecar.rs`.

- [ ] **Step 4: Route `translate_file` through the server**

In `sidecar.rs`, replace the body of `translate_file` so it delegates to the server (the process-spawn version is removed):
```rust
pub fn translate_file(input: &Path) -> Result<TranslationOutput, String> {
    crate::translation::engine_server::translate(input)
}
```
Remove the now-unused `build_command` + its `command_tests` (the server is launched by `engine_server::spawn_server`, not per-call). Keep `engine_python`/`python_dir` (now `pub`, used by engine_server).

- [ ] **Step 5: Manage server lifecycle in state + lib**

In `src-tauri/src/state.rs`, add a field to hold the server child so it is killed on drop:
```rust
use std::process::Child;
// inside AppState:
pub server: Mutex<Option<Child>>,
```
Add to `AppState`'s construction (the existing audio-thread setup stays). Initialize `server: Mutex::new(None)`. Implement `Drop` for AppState to kill the child:
```rust
impl Drop for AppState {
    fn drop(&mut self) {
        if let Ok(mut g) = self.server.lock() {
            if let Some(mut child) = g.take() { let _ = child.kill(); }
        }
    }
}
```
In `src-tauri/src/lib.rs` `run()`, spawn + await the server in a Tauri `.setup(...)` hook, storing the child in state:
```rust
.setup(|app| {
    use tauri::Manager;
    let state = app.state::<AppState>();
    match translation::engine_server::spawn_server() {
        Ok(child) => { *state.server.lock().unwrap() = Some(child); }
        Err(e) => { eprintln!("could not spawn translation server: {e}"); }
    }
    // wait for readiness off the main thread so the window still shows
    std::thread::spawn(|| {
        if let Err(e) = translation::engine_server::wait_until_ready(std::time::Duration::from_secs(120)) {
            eprintln!("translation server not ready: {e}");
        }
    });
    Ok(())
})
```
(Place `.setup` before `.run`. Keep `.manage(AppState::default())`, `.plugin(...)`, and the handler.)

- [ ] **Step 6: Verify build + tests**

Run: `cd src-tauri && cargo build` (clean) and `cargo test` (engine_server tests + json_tests + audio tests pass; command_tests removed). Report counts.

- [ ] **Step 7: Commit**

```bash
git add src-tauri/src/translation/ src-tauri/src/state.rs src-tauri/src/lib.rs src-tauri/Cargo.toml
git commit -m "feat(engine): Rust spawns persistent server, translate_file uses HTTP"
```

---

### Task 3: Manual end-to-end verification (CPU, persistence)

**Files:** none.

- [ ] **Step 1: Run the app**

Run: `pnpm tauri dev`. On startup the server spawns and loads models (first time slow). The window should appear immediately (readiness waits off-thread).

- [ ] **Step 2: Translate the same/different Spanish file 3 times**

Use the test UI. Time each translation (watch the terminal / a clock).

- [ ] **Step 3: Confirm persistence + correctness**

- The FIRST translation may take longer if models are still loading; once `/health` is ready, each translation is **~2s** (not ~20s) — this proves models stay loaded (no per-call reload).
- Output English audio plays; ES/EN text shows.
- `nvidia-smi` in another terminal shows **no** Python on the GPU (CPU-only).
- Closing the app terminates the Python server (check Task Manager: no orphaned `python.exe` from lt_engine).

- [ ] **Step 4: Note timings** in `python/SPIKE_NOTES.md` (steady-state per-request latency) for the B/C latency budget.

---

## Self-Review

**Spec coverage (against the sub-phase A design doc):**
- Persistent FastAPI server, models loaded once → Task 1 (warmup in lifespan, singletons). ✅
- `/health` + `/translate` contract → Task 1. ✅
- Rust spawns sidecar, health-check, HTTP translate → Task 2. ✅
- Lifecycle (kill on exit) → Task 2 Step 5 (AppState Drop + setup spawn). ✅
- Preserve `translate_file` signature → Task 2 Step 4 (delegates). ✅
- 127.0.0.1 only, port via env, input validation → Task 1 (uvicorn host) + Task 2 (translate guards). ✅
- CPU-only, cross-platform → Global Constraints; Task 3 verifies no GPU. ✅
- Testing: server contract (mocked), Rust parse (pure), manual persistence → Tasks 1, 2, 3. ✅

**Placeholder scan:** None. Health-check/spawn are concrete; no "handle errors" hand-waving.

**Type consistency:** `translate_audio(input_path,out_dir,src,tgt)->dict{output_wav,source_text,translated_text}` (Py) ↔ `/translate` JSON ↔ `parse_translate_response` → `TranslationOutput{output_wav,source_text,translated_text}` (Rust, reused from sidecar.rs) ↔ Tauri `TranslationFileResult`. `engine_python`/`python_dir` made `pub` and reused. `spawn_server`/`wait_until_ready`/`translate`/`base_url` consistent.

## Delivery

Branch: `feat/streaming-a-persistent-server`. On completion: PR → main (squash), trunk-based flow. Deferred items (temp-dir cleanup, packaging path) still tracked for later phases.
