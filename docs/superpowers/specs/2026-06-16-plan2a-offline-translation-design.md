# Plan 2a — Offline Translation Engine (Hibiki-Zero) Design

**Date:** 2026-06-16
**Status:** Approved for planning
**Depends on:** Plan 1 (walking skeleton audio passthrough) — DONE
**Part of:** Plan 2 (Hibiki integration), split into 2a (offline) and 2b (live streaming)

---

## 1. Summary

Plan 2a validates the translation engine in isolation, offline. Inside the app, the user
picks a Spanish audio file; Hibiki-Zero translates it to English **in the user's own voice**
(zero-shot voice transfer) and the app plays the result and shows the translated text.

No microphone, no live streaming, no virtual cable — those belong to Plan 2b. The single
purpose of 2a is to de-risk the engine: prove that Hibiki-Zero loads, runs within the user's
GPU budget, and produces an acceptable Spanish→English translation **before** investing in
the real-time integration.

## 2. Goals & Non-Goals

### Goals
- Run Hibiki-Zero 3B (PyTorch) locally as a Python sidecar, driven by the Tauri/Rust app.
- Translate a Spanish audio file to English with voice transfer; surface audio + text.
- Prove feasibility on the target hardware (NVIDIA, 8 GB VRAM) early, via a model spike.
- A clean, testable Rust↔sidecar boundary that Plan 2b can reuse.

### Non-Goals (Plan 2b or later)
- Live microphone capture, streaming, virtual-cable output.
- Low-latency / real-time behavior.
- Packaging Python for end users (bundling) — dev-managed environment for now.
- macOS support for the engine (Hibiki-Zero is NVIDIA/CUDA only).
- Language pairs beyond Spanish→English in the UI (engine also does FR/PT/DE→EN).

## 3. Key Technical Decisions

### 3.1 Engine: Hibiki-Zero 3B PyTorch as a Python sidecar
Spanish is only available in Hibiki-Zero, which ships **only** in `pytorch-bf16` (and
`mlx-bf16`) — there is no Rust build. So the engine runs in Python, as a separate process
the Rust app controls. Embedded Rust/Candle (Plan 1's original premise) only covers the
French→English models and is therefore not used for the Spanish MVP.

### 3.2 Runtime via `uv` and the `hibiki-zero` CLI
Hibiki-Zero provides its own package and CLI. Offline translation:
```
uv run hibiki-zero generate --file <input_audio>
```
`uv` manages Python 3.13 + dependencies in a project-local environment (`python/`), so the
dev does not hand-manage venvs. Model weights (`kyutai/hibiki-zero-3b-pytorch-bf16`) are
downloaded and cached on first run.

### 3.3 Hardware requirement (hard)
NVIDIA GPU, 8 GB VRAM minimum (12 GB safe), no CPU fallback. Target dev/test machine has
8 GB — the spike (Task 0) must confirm the 3B model fits and runs there.

## 4. Risk-First Validation: the Model Spike (most important part)

Before any integration code, a manual spike validates the riskiest assumption:

1. Install `uv`; run `uvx -p 3.13 hibiki-zero generate --file <spanish_sample>` (or the repo's
   local-dev form), letting it download the 3B weights.
2. Confirm: it runs within 8 GB VRAM (no OOM), produces an English audio file, the voice
   resembles the speaker, and the translation is intelligible.
3. Record the exact working command, the model download size, VRAM peak, and run time.

If the spike fails (OOM, unusable quality), we stop and reconsider (smaller model, quantization,
or revisiting the engine) — having spent ~30 minutes instead of weeks. Only after the spike
passes do we build the integration below.

## 5. Architecture

```
UI (test screen):  [choose file] → [Translate] → [▶ play output] + translated text
        │  invoke("translate_file", { path })
        ▼
Rust  translation/sidecar.rs
        │  std::process::Command
        │  uv run hibiki-zero generate --file <in>   (cwd = python/)
        ▼
Python sidecar (uv-managed): Hibiki-Zero 3B (PyTorch/CUDA)
        │  writes out.wav (24 kHz) ; prints translated text to stdout
        ▼
Rust captures output wav path + stdout text → returns TranslationOutput
        ▼
UI plays the wav and shows the text
```

## 6. Components

| Component | Responsibility | Notes |
|---|---|---|
| **`python/` env** | `pyproject.toml` declaring `hibiki-zero`, managed by `uv` | `uv sync` for setup; documented invocation |
| **`translation/sidecar.rs`** | Spawn the sidecar, pass input path, await completion, capture output wav path + stdout text, map failures to clear errors | Narrow contract: `translate_file(input: &Path) -> Result<TranslationOutput, String>` |
| **`TranslationOutput`** | `{ output_wav: PathBuf, text: String }` | Shared type returned to the command |
| **Tauri command `translate_file`** | Bridge UI → sidecar; runs off the UI thread | Lives in `lib.rs` |
| **Test UI screen** | File picker + Translate button + audio player + text area | Separate from the Plan 1 passthrough UI |

**Isolation:** `sidecar.rs` knows only "given an input path, produce a translated wav + text".
It does not know about audio devices or the passthrough. Plan 2b will reuse this boundary
(swapping the offline CLI for the streaming server) without changing consumers.

## 7. Data Flow

1. User selects an input audio file (any format the CLI accepts; it decodes/resamples to 24 kHz mono internally).
2. `translate_file` spawns `uv run hibiki-zero generate --file <in>` with a known output location.
3. The sidecar downloads the model on first run (cached afterwards), translates, writes the
   English wav (24 kHz, voice-transferred) and prints the translated text to stdout.
4. Rust verifies the output wav exists, captures the text, returns `TranslationOutput`.
5. UI plays the wav and displays the text.

## 8. Error Handling

| Situation | Behavior |
|---|---|
| `uv` or `hibiki-zero` not installed | Detected; clear message with the one-time setup command |
| Model not yet downloaded | Surface "downloading model…" state so the app doesn't look hung |
| OOM / insufficient VRAM | Capture the sidecar's error; show it plainly (note 8 GB is the floor) |
| No NVIDIA GPU | Explicit requirement message |
| Sidecar non-zero exit / no output file | Return the captured stderr as an actionable error; never silently succeed |

## 9. Requirements

- Windows or Linux + NVIDIA GPU, 8 GB+ VRAM. (macOS not supported for the engine.)
- `uv` installed; Python 3.13 managed by `uv`.

## 10. Testing Strategy

- **Unit (`cargo test`) — mock sidecar:** a small stub script that mimics the `hibiki-zero
  generate` interface (consumes an input path, writes a fixture wav, prints fixed text). Tests
  `sidecar.rs` invocation, output-path handling, stdout parsing, and error mapping — fast,
  deterministic, no GPU, no model. This is the correct level for automated tests because the
  real model is slow, GPU-bound, and non-deterministic (can't assert on generated audio).
- **Model spike (Task 0) — real model, manual, run once up front:** validates the engine
  actually works on the target machine (see §4). De-risks before integration.
- **Manual integration verification — real model, end of plan:** translate a real Spanish
  sample through the full app (UI → sidecar → playback) and confirm by ear: English output,
  voice resemblance, intelligible translation.

## 11. Relationship to Plan 2b

2a produces a validated engine and a clean `sidecar` boundary. 2b replaces the offline
`generate` CLI with the streaming server (`hibiki-zero serve`), connects it to the live mic
(Plan 1's capture) and the virtual cable (Plan 1's output), and addresses latency and the
real-time inference thread. 2a deliberately carries none of that complexity.
