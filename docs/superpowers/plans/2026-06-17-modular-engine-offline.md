# Modular Translation Engine (Offline, CPU) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Translate a Spanish audio file to an English audio file (generic voice), fully on CPU, via a modular Python sidecar (Parakeet STT → NLLB translation → Piper TTS), surfaced through the existing Plan 2a test UI.

**Architecture:** A Python sidecar (`lt_engine`) chains three small models on CPU and writes an output wav + a JSON with source/translated text. The Tauri/Rust app invokes it as a child process (same pattern as Plan 2a's `translate_file`), then the UI plays the audio and shows both texts. No GPU, no CUDA, stable PyTorch CPU.

**Tech Stack:** Python 3.11 (NeMo needs <3.13), `nemo_toolkit[asr]` (Parakeet), `transformers` + `sentencepiece` (NLLB-200), `piper-tts` (ONNX TTS), `uv` for the env; Tauri 2 / Rust bridge; existing TS test UI.

## Global Constraints

- **CPU-only execution.** No CUDA, no torch nightly. Install the **CPU** build of torch. The app must never require an NVIDIA GPU.
- **Cross-platform** target: Windows, macOS, Linux. Avoid OS-specific calls in the orchestrator.
- Languages first iteration: source `spa_Latn` (Spanish) → target `eng_Latn` (English). NLLB codes.
- Reuse the Plan 2a Rust sidecar pattern and test UI; do not rebuild them.
- Python env lives in `.venv-engine/` (gitignored, like `.venv-hibiki/`).
- Artifacts (code, comments, UI copy) in English.

---

### Task 0: Environment + pipeline spike (MANUAL gate — CPU, no GPU risk)

**Files:**
- Create: `python/SPIKE_NOTES.md`
- Create (throwaway, may be kept as reference): `python/spike.py`

**Interfaces:**
- Produces: documented, verified APIs for `transcribe(path)->str`, `translate(text)->str`, `synthesize(text,out)->wav`, plus install commands and CPU timings, consumed by Tasks 1–2.

- [ ] **Step 1: Create a CPU Python env**

```bash
uv venv -p 3.11 .venv-engine
source .venv-engine/Scripts/activate   # Git Bash on Windows
uv pip install "nemo_toolkit[asr]" transformers sentencepiece piper-tts soundfile
uv pip install torch --index-url https://download.pytorch.org/whl/cpu   # CPU build, stable
```
If `nemo_toolkit[asr]` fails to install on Windows CPU, STOP and report the exact error — we may pivot the STT piece (e.g. faster-whisper) before writing integration. This is the main risk this spike exists to surface.

- [ ] **Step 2: Verify each piece on a real Spanish sample, on CPU**

Write `python/spike.py` that, using `sample_es.wav`:
1. Loads Parakeet `nvidia/parakeet-tdt-0.6b-v3` and transcribes → prints Spanish text. Record the EXACT call that works and the return shape (NeMo `transcribe()` return type varies by version).
2. Loads NLLB `facebook/nllb-200-distilled-600M`, sets `tokenizer.src_lang="spa_Latn"`, generates with `forced_bos_token_id` for `eng_Latn` → prints English text.
3. Runs Piper (CLI `piper -m <voice>.onnx -f out.wav` with text on stdin, or the Python API) with an English voice (e.g. `en_US-lessac-medium`) → writes `out.wav`.

- [ ] **Step 3: Record findings + gate**

Write `python/SPIKE_NOTES.md`: exact install commands, the working API call for each stage (signatures + return shapes), the English Piper voice used + how it's obtained, CPU load+run times, and peak RAM. If any stage can't run on CPU, STOP and report. Commit:
```bash
git add python/SPIKE_NOTES.md python/spike.py
git commit -m "docs(spike): verify modular CPU pipeline (parakeet+nllb+piper) APIs and install"
```

> Tasks 1–2 use the API calls and install steps documented here. If reality differs from the skeleton code below (NeMo return shape, Piper invocation), adjust to match SPIKE_NOTES.md — the structure (3 stages, file→file, JSON sidecar contract) is unaffected.

---

### Task 1: Python orchestrator `lt_engine` (file → wav + text JSON)

**Files:**
- Create: `python/lt_engine/__init__.py`
- Create: `python/lt_engine/pipeline.py`
- Create: `python/lt_engine/__main__.py`
- Create: `python/tests/test_langcodes.py`

**Interfaces:**
- Produces (CLI contract consumed by the Rust bridge in Task 2):
  `python -m lt_engine --file <in> --out-dir <dir> [--src spa_Latn] [--tgt eng_Latn]`
  → writes `<dir>/output.wav` and `<dir>/result.json` = `{"source_text": "...", "translated_text": "..."}`; exit 0 on success, non-zero + stderr on failure.

- [ ] **Step 1: TDD the pure language-code validation**

`python/lt_engine/pipeline.py`:
```python
"""Modular offline translation pipeline: STT -> MT -> TTS (CPU)."""
from __future__ import annotations

# NLLB uses FLORES-200 codes like "spa_Latn", "eng_Latn".
_NLLB_CODE = {"es": "spa_Latn", "en": "eng_Latn"}

def normalize_lang(code: str) -> str:
    """Accept either a short code ('es') or a full NLLB code ('spa_Latn')."""
    if "_" in code:
        return code
    if code in _NLLB_CODE:
        return _NLLB_CODE[code]
    raise ValueError(f"unknown language code: {code}")
```

`python/tests/test_langcodes.py`:
```python
from lt_engine.pipeline import normalize_lang
import pytest

def test_short_code_maps_to_nllb():
    assert normalize_lang("es") == "spa_Latn"
    assert normalize_lang("en") == "eng_Latn"

def test_full_code_passthrough():
    assert normalize_lang("por_Latn") == "por_Latn"

def test_unknown_short_code_raises():
    with pytest.raises(ValueError):
        normalize_lang("zz")
```

- [ ] **Step 2: Run the test (RED → GREEN)**

Run: `cd python && ../.venv-engine/Scripts/python -m pytest tests/test_langcodes.py -v`
Expected: PASS (stub `normalize_lang` to `return code` first to see RED if desired).

- [ ] **Step 3: Add the three stages (use the APIs verified in Task 0)**

Append to `pipeline.py` — adjust the marked calls to match `SPIKE_NOTES.md`:
```python
def transcribe(audio_path: str) -> str:
    from nemo.collections.asr.models import ASRModel
    model = ASRModel.from_pretrained("nvidia/parakeet-tdt-0.6b-v3", map_location="cpu")
    result = model.transcribe([audio_path])          # ADJUST per SPIKE_NOTES return shape
    item = result[0]
    return getattr(item, "text", item)                # str either way

def translate(text: str, src: str, tgt: str) -> str:
    from transformers import AutoTokenizer, AutoModelForSeq2SeqLM
    name = "facebook/nllb-200-distilled-600M"
    tok = AutoTokenizer.from_pretrained(name)
    model = AutoModelForSeq2SeqLM.from_pretrained(name)
    tok.src_lang = src
    inputs = tok(text, return_tensors="pt")
    gen = model.generate(**inputs, forced_bos_token_id=tok.convert_tokens_to_ids(tgt), max_length=512)
    return tok.batch_decode(gen, skip_special_tokens=True)[0]

def synthesize(text: str, out_wav: str) -> None:
    # Piper CLI verified in Task 0; text on stdin, ONNX voice model.
    import subprocess
    subprocess.run(
        ["piper", "-m", _piper_voice_path(), "-f", out_wav],
        input=text.encode("utf-8"), check=True,
    )

def _piper_voice_path() -> str:
    import os
    return os.environ.get("PIPER_VOICE", "voices/en_US-lessac-medium.onnx")
```

- [ ] **Step 4: Add the CLI entrypoint**

`python/lt_engine/__main__.py`:
```python
import argparse, json, os, sys
from .pipeline import normalize_lang, transcribe, translate, synthesize

def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--file", required=True)
    ap.add_argument("--out-dir", required=True)
    ap.add_argument("--src", default="es")
    ap.add_argument("--tgt", default="en")
    args = ap.parse_args()
    if not os.path.isfile(args.file):
        print(f"input not found: {args.file}", file=sys.stderr); return 2
    os.makedirs(args.out_dir, exist_ok=True)
    src, tgt = normalize_lang(args.src), normalize_lang(args.tgt)
    source_text = transcribe(args.file)
    translated_text = translate(source_text, src, tgt)
    out_wav = os.path.join(args.out_dir, "output.wav")
    synthesize(translated_text, out_wav)
    with open(os.path.join(args.out_dir, "result.json"), "w", encoding="utf-8") as f:
        json.dump({"source_text": source_text, "translated_text": translated_text}, f, ensure_ascii=False)
    return 0

if __name__ == "__main__":
    raise SystemExit(main())
```
`python/lt_engine/__init__.py`: empty.

- [ ] **Step 5: Verify the CLI on the real sample (manual, CPU)**

Run: `cd python && ../.venv-engine/Scripts/python -m lt_engine --file sample_es.wav --out-dir _out`
Expected: `_out/output.wav` plays English; `_out/result.json` has both texts. (Slow on CPU — that's fine.)

- [ ] **Step 6: Commit**

```bash
git add python/lt_engine python/tests
git commit -m "feat(engine): modular CPU pipeline orchestrator (parakeet+nllb+piper)"
```

---

### Task 2: Rust bridge — point the sidecar at `lt_engine`

**Files:**
- Modify: `src-tauri/src/translation/sidecar.rs`
- Modify: `src-tauri/src/lib.rs`

**Interfaces:**
- Consumes: the `lt_engine` CLI contract from Task 1 (`--file`, `--out-dir`, `output.wav`, `result.json`).
- Produces: `translate_file(&Path) -> Result<TranslationOutput, String>` where
  `TranslationOutput { output_wav: PathBuf, source_text: String, translated_text: String }`;
  Tauri command returns `{ output_wav, source_text, translated_text }`.

- [ ] **Step 1: Extend TranslationOutput + result parsing (TDD)**

In `sidecar.rs`, replace the `TranslationOutput` struct and add JSON parsing of `result.json`. Keep `pick_output_wav` but the engine now writes a fixed `output.wav`, so add a deterministic finder and a pure parser tested without the model:
```rust
#[derive(Debug, Clone, PartialEq)]
pub struct TranslationOutput {
    pub output_wav: PathBuf,
    pub source_text: String,
    pub translated_text: String,
}

/// Parses the engine's result.json content.
pub fn parse_result_json(s: &str) -> Result<(String, String), String> {
    let v: serde_json::Value = serde_json::from_str(s).map_err(|e| e.to_string())?;
    let src = v.get("source_text").and_then(|x| x.as_str()).unwrap_or("").to_string();
    let tgt = v.get("translated_text").and_then(|x| x.as_str()).unwrap_or("").to_string();
    Ok((src, tgt))
}

#[cfg(test)]
mod json_tests {
    use super::*;
    #[test]
    fn parses_both_texts() {
        let (s, t) = parse_result_json(r#"{"source_text":"hola","translated_text":"hello"}"#).unwrap();
        assert_eq!(s, "hola"); assert_eq!(t, "hello");
    }
    #[test]
    fn missing_fields_default_empty() {
        let (s, t) = parse_result_json("{}").unwrap();
        assert_eq!(s, ""); assert_eq!(t, "");
    }
    #[test]
    fn invalid_json_errors() {
        assert!(parse_result_json("not json").is_err());
    }
}
```
Add `serde_json = "1"` to `src-tauri/Cargo.toml` if not present (Tauri pulls it transitively; add explicitly to be safe).

- [ ] **Step 2: Run the parser tests (RED → GREEN)**

Run: `cd src-tauri && cargo test json_tests`
Expected: 3 pass.

- [ ] **Step 3: Update `build_command` + `translate_file` to call `lt_engine`**

Replace `build_command` and the runner body to invoke the engine venv's python with the module, and read `output.wav` + `result.json`:
```rust
fn engine_python() -> String {
    if let Ok(p) = std::env::var("LT_ENGINE_PYTHON") { return p; }
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).parent()
        .map(|p| p.to_path_buf()).unwrap_or_else(|| std::path::PathBuf::from("."));
    let rel = if cfg!(windows) { ".venv-engine/Scripts/python.exe" } else { ".venv-engine/bin/python" };
    root.join(rel).to_string_lossy().into_owned()
}

pub fn build_command(input: &Path, out_dir: &Path) -> (String, Vec<String>) {
    (engine_python(), vec![
        "-m".into(), "lt_engine".into(),
        "--file".into(), input.to_string_lossy().into_owned(),
        "--out-dir".into(), out_dir.to_string_lossy().into_owned(),
    ])
}
```
In `translate_file`, run the command (cwd = repo `python/` dir so `-m lt_engine` resolves — set `.current_dir(python_dir())`), then on success read `out_dir/output.wav` and `out_dir/result.json`:
```rust
    let wav = out_dir.join("output.wav");
    if !wav.exists() {
        let _ = std::fs::remove_dir_all(&out_dir);
        return Err("engine produced no output.wav".into());
    }
    let json = std::fs::read_to_string(out_dir.join("result.json")).unwrap_or_default();
    let (source_text, translated_text) = parse_result_json(&json).unwrap_or_default();
    Ok(TranslationOutput { output_wav: wav, source_text, translated_text })
```
Add a `python_dir()` helper mirroring `engine_python()` but returning the `python/` dir, and set it as the command's `current_dir`. Keep the `input.exists()` + extension allowlist guards and the unique temp `out_dir`.

- [ ] **Step 4: Update the Tauri command result in `lib.rs`**

```rust
#[derive(serde::Serialize)]
struct TranslationFileResult { output_wav: String, source_text: String, translated_text: String }

#[tauri::command]
async fn translate_file(input_path: String) -> Result<TranslationFileResult, String> {
    tauri::async_runtime::spawn_blocking(move || {
        translation::sidecar::translate_file(std::path::Path::new(&input_path))
    }).await.map_err(|e| e.to_string())?
      .map(|o| TranslationFileResult {
          output_wav: o.output_wav.to_string_lossy().into_owned(),
          source_text: o.source_text,
          translated_text: o.translated_text,
      })
}
```

- [ ] **Step 5: Verify build + tests**

Run: `cd src-tauri && cargo build && cargo test`
Expected: clean; json_tests + prior pure tests pass.

- [ ] **Step 6: Commit**

```bash
git add src-tauri/src/translation/sidecar.rs src-tauri/src/lib.rs src-tauri/Cargo.toml
git commit -m "feat(engine): Rust bridge invokes lt_engine and parses result.json"
```

---

### Task 3: Test UI — show source + translated text

**Files:**
- Modify: `src/translate.ts`

**Interfaces:**
- Consumes: `invoke<{ output_wav: string; source_text: string; translated_text: string }>("translate_file", { inputPath })`.

- [ ] **Step 1: Update the result handling**

In `src/translate.ts`, change the result type and display both texts. Replace the invoke block inside the toggle/translate handler:
```ts
const res = await invoke<{ output_wav: string; source_text: string; translated_text: string }>(
  "translate_file", { inputPath },
);
player.src = convertFileSrc(res.output_wav);
text.textContent = `ES: ${res.source_text}\nEN: ${res.translated_text}`;
status.textContent = "Done.";
```
Keep the existing try/catch and disabled-state logic.

- [ ] **Step 2: Verify frontend build**

Run (repo root): `pnpm build`
Expected: 0 TS errors.

- [ ] **Step 3: Commit**

```bash
git add src/translate.ts
git commit -m "feat(ui): show source and translated text from modular engine"
```

---

### Task 4: Manual end-to-end verification (CPU)

**Files:** none.

- [ ] **Step 1: Run the app**

Run: `pnpm tauri dev`

- [ ] **Step 2: Translate a Spanish sample via the UI**

Pick a Spanish `.wav`, click Translate. It runs on **CPU** — slower than GPU, but **must not touch the GPU or risk the PC**. The window must stay responsive (async command).

- [ ] **Step 3: Confirm**

- The player loads and plays English (generic voice).
- The text area shows both `ES:` (transcription) and `EN:` (translation).
- No GPU usage spike (verify with `nvidia-smi` in another terminal — the python process should NOT appear on the GPU).
- Errors (missing env, model download) show a clear message; the UI never freezes.

- [ ] **Step 4: Note outcome**

Record CPU run time + peak RAM in `python/SPIKE_NOTES.md` for the streaming-phase latency budget.

---

## Self-Review

**Spec coverage (against the modular-engine design doc):**
- CPU-first modular pipeline STT→MT→TTS → Tasks 0–2. ✅
- Parakeet / NLLB / Piper choices → Tasks 0–1. ✅
- Reuse Plan 2a sidecar + UI patterns → Tasks 2–3. ✅
- No GPU requirement, stable torch CPU → Global Constraints + Task 0 (CPU torch index) + Task 4 (verify no GPU). ✅
- Engine boundary reusable by streaming phase → Task 2 `translate_file` contract. ✅
- Source + translated text surfaced → Tasks 1 (result.json) → 2 → 3. ✅
- Roadmap items (virtual-cable routing, streaming, voice cloning, multi-language) are explicitly OUT of this iteration (design §8) — not tasks here.

**Placeholder scan:** No TBD/TODO. The two empirical unknowns (NeMo `transcribe` return shape, exact Piper CLI) are resolved by Task 0 and the integration code is marked to adjust to SPIKE_NOTES.md — not hand-waved.

**Type consistency:** `TranslationOutput { output_wav, source_text, translated_text }` (Rust) ↔ `TranslationFileResult { output_wav, source_text, translated_text }` ↔ JS `{ output_wav, source_text, translated_text }` ↔ engine `result.json { source_text, translated_text }`; CLI contract `--file/--out-dir`, `output.wav`, `result.json` consistent across Tasks 1–2. `normalize_lang`, `transcribe`, `translate`, `synthesize`, `parse_result_json`, `engine_python`, `build_command` used consistently.

## Delivery

Branch: `feat/modular-engine-offline`. On completion: PR → main (squash), per trunk-based flow. `.venv-engine/` gitignored. CI not yet configured.
