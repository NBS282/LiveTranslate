# Plan 2a — Offline Translation Engine Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Inside the LiveTranslate app, translate a Spanish audio file to English (voice-transferred) using Hibiki-Zero 3B as a Python sidecar, playing the result and showing the text — offline, NVIDIA-only.

**Architecture:** The Tauri/Rust app invokes the `hibiki-zero generate` CLI (run via `uv`) as a child process in a temporary working directory, locates the produced `.wav`, captures stdout text, and returns it to a minimal test UI. Rust never embeds the model; it orchestrates the sidecar through a narrow `translate_file` boundary that Plan 2b will reuse.

**Tech Stack:** Tauri 2, Rust (`std::process::Command`), `uv` + `hibiki-zero` (Python 3.13, PyTorch/CUDA), TypeScript frontend.

---

## Why a spike first

The riskiest assumption is that Hibiki-Zero 3B runs in 8 GB VRAM and translates Spanish acceptably. Task 0 validates that **by hand, before any integration code**. It also discovers the exact `generate` CLI interface (output location, stdout text, flags), which the README does not document. If the spike fails, we stop and reconsider after ~30 minutes, not after building the whole feature.

## File Structure

```
LiveTranslate/
├── docs/superpowers/specs/2026-06-16-plan2a-offline-translation-design.md   # the design
├── python/
│   └── SPIKE_NOTES.md                # Task 0 output: exact verified CLI interface + perf
├── src-tauri/src/
│   ├── translation/
│   │   ├── mod.rs                    # exports
│   │   └── sidecar.rs                # TranslationOutput, build_command, pick_output_wav, translate_file
│   └── lib.rs                        # add `mod translation;` + `translate_file` Tauri command
└── src/
    ├── translate.html                # minimal test screen (or a section toggled in index.html)
    └── translate.ts                  # file picker + invoke + audio player + text
```

**Isolation:** `sidecar.rs` only knows "input path → translated wav + text". It has no knowledge of audio devices or the Plan 1 passthrough. Plan 2b reuses this boundary, swapping the offline CLI for the streaming server.

---

### Task 0: Model spike (manual gate — do this before any code)

**Files:**
- Create: `python/SPIKE_NOTES.md`

- [ ] **Step 1: Ensure `uv` is installed**

Run: `uv --version`
Expected: prints a version. If missing, install uv (`winget install astral-sh.uv` on Windows) and retry.

- [ ] **Step 2: Inspect the real CLI interface**

Run: `uvx -p 3.13 hibiki-zero generate --help`
Read the actual flags. Note specifically: how the output path/name is determined, whether translated text is printed, and any source-language / hf-repo / voice-transfer flags.

- [ ] **Step 3: Translate a real Spanish sample**

Obtain or record a short Spanish `.wav` (5–15 s), e.g. `sample_es.wav`. Run:
`uvx -p 3.13 hibiki-zero generate --file sample_es.wav`
Let it download the `kyutai/hibiki-zero-3b-pytorch-bf16` weights on first run.

- [ ] **Step 4: Validate and record findings**

Confirm by ear: the output is English, intelligible, and resembles your voice. While it runs, watch VRAM (Task Manager / `nvidia-smi`). Write `python/SPIKE_NOTES.md` capturing:
- the exact working command,
- WHERE the output wav was written and its naming (cwd? next to input? a flag?),
- whether translated text appeared on stdout (and its format),
- peak VRAM, model download size, and run time,
- any extra flags discovered (source language, hf-repo, cfg/voice coefficient).

- [ ] **Step 5: Gate**

If it OOMs or quality is unusable: STOP and report — do not proceed. Otherwise commit the notes:
```bash
git add python/SPIKE_NOTES.md
git commit -m "docs(spike): record verified Hibiki-Zero generate CLI interface and perf"
```

> Tasks 2–5 assume the CLI writes the output `.wav` into the process working directory and that translated text (if any) goes to stdout. If SPIKE_NOTES.md shows otherwise (e.g. output written next to the input, or requiring an explicit `--out` flag), adjust `build_command` and the locate strategy in Task 2 to match the documented behavior — the rest of the design is unaffected.

---

### Task 1: TranslationOutput + pure output-locating logic (TDD)

**Files:**
- Create: `src-tauri/src/translation/mod.rs`
- Create: `src-tauri/src/translation/sidecar.rs`
- Modify: `src-tauri/src/lib.rs` (add `mod translation;`)

- [ ] **Step 1: Write the failing test**

Create `src-tauri/src/translation/sidecar.rs`:
```rust
use std::path::PathBuf;

/// Result of a successful offline translation.
#[derive(Debug, Clone, PartialEq)]
pub struct TranslationOutput {
    pub output_wav: PathBuf,
    pub text: String,
}

/// Picks the translated audio file from the files produced in the work dir.
/// Returns the first `.wav` (case-insensitive) found.
pub fn pick_output_wav(files: &[PathBuf]) -> Option<PathBuf> {
    files
        .iter()
        .find(|p| {
            p.extension()
                .and_then(|e| e.to_str())
                .map(|e| e.eq_ignore_ascii_case("wav"))
                .unwrap_or(false)
        })
        .cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_the_wav_among_other_files() {
        let files = vec![
            PathBuf::from("log.txt"),
            PathBuf::from("out_en.wav"),
        ];
        assert_eq!(pick_output_wav(&files), Some(PathBuf::from("out_en.wav")));
    }

    #[test]
    fn case_insensitive_extension() {
        let files = vec![PathBuf::from("OUT.WAV")];
        assert_eq!(pick_output_wav(&files), Some(PathBuf::from("OUT.WAV")));
    }

    #[test]
    fn none_when_no_wav() {
        let files = vec![PathBuf::from("a.txt"), PathBuf::from("b.mp3")];
        assert_eq!(pick_output_wav(&files), None);
    }
}
```

Create `src-tauri/src/translation/mod.rs`:
```rust
pub mod sidecar;
```

Add to the top of `src-tauri/src/lib.rs` (alongside `mod audio;` and `mod state;`):
```rust
mod translation;
```

- [ ] **Step 2: Run test to verify it fails first**

To see RED, temporarily change `pick_output_wav`'s body to `None`, then run:
`cd src-tauri && cargo test pick_output_wav`
Expected: `picks_the_wav_among_other_files` and `case_insensitive_extension` FAIL. Restore the real body.

- [ ] **Step 3: Confirm GREEN**

Run: `cd src-tauri && cargo test pick_output_wav`
Expected: 3 passed.

- [ ] **Step 4: Commit**

```bash
git add src-tauri/src/translation/ src-tauri/src/lib.rs
git commit -m "feat(translation): add TranslationOutput and output wav selection"
```

---

### Task 2: Build the sidecar command + run it

**Files:**
- Modify: `src-tauri/src/translation/sidecar.rs`

- [ ] **Step 1: Add the command builder with a test (TDD)**

Append to `src-tauri/src/translation/sidecar.rs`:
```rust
use std::path::Path;

/// Builds the (program, args) to run the offline translator via uv.
/// Adjust here if Task 0's SPIKE_NOTES.md documented different flags.
pub fn build_command(input: &Path) -> (String, Vec<String>) {
    (
        "uvx".to_string(),
        vec![
            "-p".to_string(),
            "3.13".to_string(),
            "hibiki-zero".to_string(),
            "generate".to_string(),
            "--file".to_string(),
            input.to_string_lossy().into_owned(),
        ],
    )
}

#[cfg(test)]
mod command_tests {
    use super::*;

    #[test]
    fn builds_uvx_generate_command() {
        let (program, args) = build_command(Path::new("sample_es.wav"));
        assert_eq!(program, "uvx");
        assert_eq!(
            args,
            vec![
                "-p", "3.13", "hibiki-zero", "generate", "--file", "sample_es.wav"
            ]
        );
    }
}
```

- [ ] **Step 2: Run the builder test (RED then GREEN)**

Run: `cd src-tauri && cargo test builds_uvx_generate_command`
Expected: PASS (stub to a wrong program first if you want to see RED).

- [ ] **Step 3: Implement the runner (integration; verified manually in Task 5)**

Append to `src-tauri/src/translation/sidecar.rs`:
```rust
use std::process::Command;

/// Translates `input` to English audio + text using the Hibiki-Zero sidecar.
/// Runs the CLI in a fresh temp working directory so the produced wav is easy
/// to locate regardless of the CLI's output naming.
pub fn translate_file(input: &Path) -> Result<TranslationOutput, String> {
    if !input.exists() {
        return Err(format!("input file not found: {}", input.display()));
    }
    let work = std::env::temp_dir().join(format!("livetranslate-tr-{}", std::process::id()));
    std::fs::create_dir_all(&work).map_err(|e| e.to_string())?;

    let (program, args) = build_command(input);
    let output = Command::new(&program)
        .args(&args)
        .current_dir(&work)
        .output()
        .map_err(|e| format!("failed to start translator '{program}': {e}. Is uv installed?"))?;

    if !output.status.success() {
        return Err(format!(
            "translation failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }

    let produced: Vec<PathBuf> = std::fs::read_dir(&work)
        .map_err(|e| e.to_string())?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .collect();
    let output_wav = pick_output_wav(&produced)
        .ok_or("translator produced no .wav output")?;
    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();

    Ok(TranslationOutput { output_wav, text })
}
```

- [ ] **Step 4: Verify it compiles**

Run: `cd src-tauri && cargo build`
Expected: builds clean (unused-warnings on `translate_file` are fine until Task 3 wires it).

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/translation/sidecar.rs
git commit -m "feat(translation): build and run the hibiki-zero sidecar command"
```

---

### Task 3: Tauri command `translate_file`

**Files:**
- Modify: `src-tauri/src/lib.rs`

- [ ] **Step 1: Add the command**

In `src-tauri/src/lib.rs`, add:
```rust
#[tauri::command]
fn translate_file(input_path: String) -> Result<TranslationFileResult, String> {
    let out = translation::sidecar::translate_file(std::path::Path::new(&input_path))?;
    Ok(TranslationFileResult {
        output_wav: out.output_wav.to_string_lossy().into_owned(),
        text: out.text,
    })
}

#[derive(serde::Serialize)]
struct TranslationFileResult {
    output_wav: String,
    text: String,
}
```
(`serde` is already available via Tauri.) Register it in the existing handler:
```rust
.invoke_handler(tauri::generate_handler![
    get_output_devices,
    start_passthrough,
    stop_passthrough,
    translate_file
])
```

- [ ] **Step 2: Verify it compiles**

Run: `cd src-tauri && cargo build`
Expected: builds clean.

- [ ] **Step 3: Commit**

```bash
git add src-tauri/src/lib.rs
git commit -m "feat(translation): expose translate_file Tauri command"
```

---

### Task 4: Minimal test UI (file picker + translate + play + text)

**Files:**
- Modify: `index.html`
- Create: `src/translate.ts`
- Modify: `src-tauri/Cargo.toml` (add the Tauri dialog plugin) and `src-tauri/src/lib.rs` (register it)
- Modify: `src-tauri/capabilities/default.json` (allow dialog + fs read of the output)

- [ ] **Step 1: Add the Tauri dialog plugin (for the file picker)**

In `src-tauri/Cargo.toml` `[dependencies]`:
```toml
tauri-plugin-dialog = "2"
```
In `src-tauri/src/lib.rs` `run()`, add the plugin before `.manage(...)`:
```rust
.plugin(tauri_plugin_dialog::init())
```
Install the JS side from the repo root:
```bash
pnpm add @tauri-apps/plugin-dialog
```

- [ ] **Step 2: Add a translate section to `index.html`**

Inside `<main>`, append:
```html
<hr />
<section>
  <h2>Offline translation test</h2>
  <button id="pick">Choose Spanish audio…</button>
  <span id="chosen">No file</span>
  <button id="translate" disabled>Translate</button>
  <p id="tr-status"></p>
  <audio id="player" controls></audio>
  <pre id="tr-text"></pre>
</section>
<script type="module" src="/src/translate.ts"></script>
```

- [ ] **Step 3: Wire the UI**

Create `src/translate.ts`:
```ts
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import { convertFileSrc } from "@tauri-apps/api/core";

const pick = document.querySelector<HTMLButtonElement>("#pick")!;
const chosen = document.querySelector<HTMLSpanElement>("#chosen")!;
const translate = document.querySelector<HTMLButtonElement>("#translate")!;
const status = document.querySelector<HTMLParagraphElement>("#tr-status")!;
const player = document.querySelector<HTMLAudioElement>("#player")!;
const text = document.querySelector<HTMLPreElement>("#tr-text")!;

let inputPath: string | null = null;

pick.addEventListener("click", async () => {
  const selected = await open({
    multiple: false,
    filters: [{ name: "Audio", extensions: ["wav", "mp3", "flac", "ogg", "m4a"] }],
  });
  if (typeof selected === "string") {
    inputPath = selected;
    chosen.textContent = selected;
    translate.disabled = false;
  }
});

translate.addEventListener("click", async () => {
  if (!inputPath) return;
  translate.disabled = true;
  status.textContent = "Translating… (first run downloads the model)";
  try {
    const res = await invoke<{ output_wav: string; text: string }>("translate_file", {
      inputPath,
    });
    player.src = convertFileSrc(res.output_wav);
    text.textContent = res.text || "(no text returned)";
    status.textContent = "Done.";
  } catch (e) {
    status.textContent = `Error: ${e}`;
  } finally {
    translate.disabled = false;
  }
});
```

- [ ] **Step 4: Allow dialog + reading the output file**

In `src-tauri/capabilities/default.json`, add to the `permissions` array:
```json
"dialog:allow-open",
"core:path:default",
"fs:allow-read-file"
```
(If `convertFileSrc` needs an asset-protocol scope, follow the error message Tauri prints; the spike/run will reveal the exact permission string for this Tauri version.)

- [ ] **Step 5: Verify build**

Run from repo root: `pnpm build` then `cd src-tauri && cargo build`
Expected: both succeed.

- [ ] **Step 6: Commit**

```bash
git add index.html src/translate.ts src-tauri/Cargo.toml src-tauri/src/lib.rs src-tauri/capabilities/default.json package.json pnpm-lock.yaml
git commit -m "feat(ui): offline translation test screen"
```

---

### Task 5: Manual end-to-end verification (real model)

**Files:** none (verification only)

- [ ] **Step 1: Run the app**

Run: `pnpm tauri dev` (first model run is slow; subsequent runs use the cached weights).

- [ ] **Step 2: Translate a Spanish sample via the UI**

Click "Choose Spanish audio…", pick a Spanish `.wav`, click "Translate". Wait for "Done."

- [ ] **Step 3: Confirm the result**

- The audio player plays English output that resembles your voice.
- The text area shows the translation (or "(no text returned)" if the CLI emits none — acceptable for 2a).
- Status never gets stuck; errors (OOM, missing uv) show a clear message.

- [ ] **Step 4: Note outcome**

Record VRAM/run-time in `python/SPIKE_NOTES.md` if they differ from the spike. This informs Plan 2b's latency expectations.

---

## Self-Review

**Spec coverage (against the design doc):**
- Engine as Python sidecar via uv/`hibiki-zero generate` → Tasks 0, 2. ✅
- Risk-first model spike → Task 0. ✅
- `sidecar.rs` narrow boundary (`translate_file` → wav + text) → Tasks 1–2. ✅
- Tauri command bridge → Task 3. ✅
- Test UI (picker + play + text) → Task 4. ✅
- Error handling (missing uv, OOM, no output) → Task 2 (`translate_file` error paths) + Task 4 (UI try/catch). ✅
- Mock/pure unit tests vs manual real-model verification → Tasks 1–2 (pure tests), Task 5 (manual). ✅
- Requirements NVIDIA/uv → Task 0 gate. ✅

**Placeholder scan:** No TBD/TODO. The two CLI-interface unknowns (output location, text-on-stdout) are explicitly resolved by Task 0 and absorbed by the temp-dir + `pick_output_wav` strategy; the Task 0 note tells the implementer exactly what to adjust if reality differs. Not a hand-wave.

**Type consistency:** `TranslationOutput { output_wav: PathBuf, text: String }`, `pick_output_wav`, `build_command`, `translate_file` (Rust) and the serialized `TranslationFileResult { output_wav: String, text: String }` + the JS `{ output_wav, text }` and command name `translate_file` with arg `inputPath`↔`input_path` (Tauri camelCase) are consistent across tasks.

## Delivery

Branch: `feat/plan2a-offline-translation`. On completion: PR → main (squash), per the project's trunk-based flow. CI not yet configured (Plan 4); merge on manual approval after Task 5 passes.
