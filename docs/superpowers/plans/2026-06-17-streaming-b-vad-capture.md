# Streaming Sub-phase B — VAD + Continuous Capture Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A "live listen" mode: continuously capture the mic, segment speech into phrases with VAD, translate each via the persistent server, and show ES/EN text live in the UI.

**Architecture:** A producer thread (cpal input → resample 16 kHz → webrtc-vad → pure Segmenter) pushes closed phrase segments to a channel; a worker thread writes each to a temp wav, calls `engine_server::translate`, and emits a Tauri `"phrase"` event the UI appends. Capture and translation run on separate threads so a ~2s translation never drops capture.

**Tech Stack:** Rust `cpal` (capture, Plan 1), `webrtc-vad`, `rubato` (resample), existing `engine_server` (Sub-phase A) + `reqwest`; Tauri 2 events; TS UI.

## Global Constraints

- **CPU-only**, cross-platform (Windows/macOS/Linux). No GPU.
- VAD operates on **16 kHz mono i16, 30 ms frames** (480 samples/frame). Parakeet is also 16 kHz.
- Reuse Sub-phase A server (`engine_server::translate`) and Plan 1 capture patterns. Do not rebuild them.
- Artifacts in English. Trunk-based: branch `feat/streaming-b-vad-capture`, PR+squash.

---

### Task 1: Pure phrase segmentation logic (TDD)

**Files:**
- Create: `src-tauri/src/translation/segmenter.rs`
- Modify: `src-tauri/src/translation/mod.rs` (add `pub mod segmenter;`)
- Modify: `src-tauri/Cargo.toml` (add `webrtc-vad`, `rubato` — used in Task 2; declaring now is fine)

**Interfaces:**
- Produces: `Segmenter::new(silence_close_frames: u32, min_voiced_frames: u32) -> Segmenter` and `Segmenter::push(&mut self, frame: &[i16], voiced: bool) -> Option<Vec<i16>>` (returns the closed segment's samples when a phrase ends), used by Task 2. Constant `FRAME_SAMPLES_16K: usize = 480`.

- [ ] **Step 1: Write the failing tests**

Create `src-tauri/src/translation/segmenter.rs`:
```rust
/// Samples per 30ms frame at 16 kHz mono.
pub const FRAME_SAMPLES_16K: usize = 480;

/// Closes a speech segment after enough trailing silence; drops too-short segments.
/// Fed one fixed-size frame at a time with its VAD voiced/unvoiced flag.
pub struct Segmenter {
    silence_close: u32,   // # of consecutive silence frames that close a phrase
    min_voiced: u32,      // minimum voiced frames for a segment to be emitted
    in_speech: bool,
    voiced_count: u32,
    trailing_silence: u32,
    buf: Vec<i16>,
}

impl Segmenter {
    pub fn new(silence_close: u32, min_voiced: u32) -> Self {
        Self { silence_close, min_voiced, in_speech: false, voiced_count: 0, trailing_silence: 0, buf: Vec::new() }
    }

    /// Push one frame. Returns Some(samples) when a phrase closes (and passes the min-length gate).
    pub fn push(&mut self, frame: &[i16], voiced: bool) -> Option<Vec<i16>> {
        if voiced {
            self.in_speech = true;
            self.trailing_silence = 0;
            self.voiced_count += 1;
            self.buf.extend_from_slice(frame);
            return None;
        }
        if !self.in_speech {
            return None; // silence outside speech: ignore
        }
        // trailing silence inside a phrase: keep it in the buffer, count it
        self.buf.extend_from_slice(frame);
        self.trailing_silence += 1;
        if self.trailing_silence < self.silence_close {
            return None;
        }
        // close the phrase
        let segment = std::mem::take(&mut self.buf);
        let voiced = self.voiced_count;
        self.in_speech = false;
        self.voiced_count = 0;
        self.trailing_silence = 0;
        if voiced >= self.min_voiced {
            Some(segment)
        } else {
            None // too short → discard
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn frame() -> Vec<i16> { vec![0i16; FRAME_SAMPLES_16K] }

    #[test]
    fn closes_phrase_after_silence() {
        let mut s = Segmenter::new(3, 2); // 3 silence frames close; need >=2 voiced
        assert!(s.push(&frame(), true).is_none());   // voiced
        assert!(s.push(&frame(), true).is_none());   // voiced (2)
        assert!(s.push(&frame(), false).is_none());  // silence 1
        assert!(s.push(&frame(), false).is_none());  // silence 2
        let seg = s.push(&frame(), false);           // silence 3 → close
        assert!(seg.is_some());
        // 2 voiced + 3 silence frames buffered
        assert_eq!(seg.unwrap().len(), 5 * FRAME_SAMPLES_16K);
    }

    #[test]
    fn drops_too_short_segment() {
        let mut s = Segmenter::new(2, 3); // need >=3 voiced
        s.push(&frame(), true);                       // 1 voiced
        s.push(&frame(), false);                      // silence 1
        let seg = s.push(&frame(), false);            // silence 2 → close, but only 1 voiced
        assert!(seg.is_none());                       // dropped
    }

    #[test]
    fn ignores_silence_outside_speech() {
        let mut s = Segmenter::new(2, 1);
        assert!(s.push(&frame(), false).is_none());
        assert!(s.push(&frame(), false).is_none());
    }

    #[test]
    fn second_phrase_after_first_closes() {
        let mut s = Segmenter::new(2, 1);
        s.push(&frame(), true);
        s.push(&frame(), false);
        assert!(s.push(&frame(), false).is_some());   // phrase 1 closes
        // new phrase
        s.push(&frame(), true);
        s.push(&frame(), false);
        assert!(s.push(&frame(), false).is_some());   // phrase 2 closes
    }
}
```
Add `pub mod segmenter;` to `src-tauri/src/translation/mod.rs`. In `Cargo.toml`: `webrtc-vad = "0.4"` and `rubato = "0.15"`.

- [ ] **Step 2: Run tests (RED→GREEN)**

Run: `cd src-tauri && cargo test segmenter`
Expected: 4 tests pass. (To see RED, stub `push` to `None`, then restore.)

- [ ] **Step 3: Commit**

```bash
git add src-tauri/src/translation/segmenter.rs src-tauri/src/translation/mod.rs src-tauri/Cargo.toml
git commit -m "feat(live): pure phrase segmentation logic with TDD"
```

---

### Task 2: Live capture pipeline (cpal + VAD + worker + commands)

**Files:**
- Create: `src-tauri/src/translation/live.rs`
- Modify: `src-tauri/src/translation/mod.rs` (add `pub mod live;`)
- Modify: `src-tauri/src/state.rs` (live-mode stop flag + thread handles)
- Modify: `src-tauri/src/lib.rs` (register `start_live_translation` / `stop_live_translation`)

**Interfaces:**
- Consumes: `engine_server::translate(&Path)`, `segmenter::{Segmenter, FRAME_SAMPLES_16K}`.
- Produces: Tauri commands `start_live_translation(device_name: String, app: AppHandle, state: State<AppState>) -> Result<(), String>` and `stop_live_translation(state: State<AppState>)`; emits event `"phrase"` with `{ source_text: String, translated_text: String, error: Option<String> }`.

- [ ] **Step 1: Implement live.rs (capture → resample → VAD → segmenter → queue → worker → emit)**

Create `src-tauri/src/translation/live.rs`:
```rust
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use serde::Serialize;
use tauri::{AppHandle, Emitter};
use crate::translation::segmenter::{Segmenter, FRAME_SAMPLES_16K};

#[derive(Clone, Serialize)]
pub struct PhraseEvent {
    pub source_text: String,
    pub translated_text: String,
    pub error: Option<String>,
}

/// Handle to a running live session: a stop flag the producer/worker observe.
pub struct LiveSession {
    pub stop: Arc<AtomicBool>,
}

/// Writes 16 kHz mono i16 samples to a temp wav and returns its path.
fn write_segment_wav(samples: &[i16]) -> Result<PathBuf, String> {
    let unique = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos()).unwrap_or(0);
    let path = std::env::temp_dir().join(format!("livetranslate-seg-{}-{}.wav", std::process::id(), unique));
    let spec = hound::WavSpec { channels: 1, sample_rate: 16_000, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
    let mut w = hound::WavWriter::create(&path, spec).map_err(|e| e.to_string())?;
    for s in samples { w.write_sample(*s).map_err(|e| e.to_string())?; }
    w.finalize().map_err(|e| e.to_string())?;
    Ok(path)
}

/// Worker: consume segments, translate, emit a phrase event each.
fn run_worker(rx: Receiver<Vec<i16>>, app: AppHandle, stop: Arc<AtomicBool>) {
    while !stop.load(Ordering::Relaxed) {
        match rx.recv_timeout(std::time::Duration::from_millis(250)) {
            Ok(samples) => {
                let evt = match write_segment_wav(&samples).and_then(|p| {
                    let r = crate::translation::engine_server::translate(&p);
                    let _ = std::fs::remove_file(&p);
                    r
                }) {
                    Ok(out) => PhraseEvent { source_text: out.source_text, translated_text: out.translated_text, error: None },
                    Err(e) => PhraseEvent { source_text: String::new(), translated_text: String::new(), error: Some(e) },
                };
                let _ = app.emit("phrase", evt);
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
            Err(_) => break,
        }
    }
}

/// Starts capture on `device_name`, returns a LiveSession (stop flag).
pub fn start(device_name: &str, app: AppHandle) -> Result<LiveSession, String> {
    let host = cpal::default_host();
    let device = host.input_devices().map_err(|e| e.to_string())?
        .find(|d| d.name().map(|n| n == device_name).unwrap_or(false))
        .or_else(|| host.default_input_device())
        .ok_or("no input device")?;
    let config = device.default_input_config().map_err(|e| e.to_string())?;
    let in_rate = config.sample_rate().0 as usize;
    let channels = config.channels() as usize;

    let (seg_tx, seg_rx): (Sender<Vec<i16>>, Receiver<Vec<i16>>) = std::sync::mpsc::channel();
    let stop = Arc::new(AtomicBool::new(false));

    // worker thread
    {
        let app = app.clone();
        let stop = stop.clone();
        std::thread::spawn(move || run_worker(seg_rx, app, stop));
    }

    // producer: cpal input stream → resample to 16k → 30ms frames → webrtc-vad → segmenter
    let stop_prod = stop.clone();
    std::thread::spawn(move || {
        if let Err(e) = run_producer(device, in_rate, channels, seg_tx, stop_prod) {
            eprintln!("live producer error: {e}");
        }
    });

    Ok(LiveSession { stop })
}

fn run_producer(
    device: cpal::Device,
    in_rate: usize,
    channels: usize,
    seg_tx: Sender<Vec<i16>>,
    stop: Arc<AtomicBool>,
) -> Result<(), String> {
    use webrtc_vad::{Vad, SampleRate, VadMode};
    // 700ms silence closes; ~300ms (10 frames) min voiced.
    let mut segmenter = Segmenter::new(23, 10);
    let mut vad = Vad::new_with_rate_and_mode(SampleRate::Rate16kHz, VadMode::Quality);

    // shared 16k mono f32 ring fed by the cpal callback
    let (samp_tx, samp_rx) = std::sync::mpsc::channel::<Vec<f32>>();
    let config: cpal::StreamConfig = device.default_input_config().map_err(|e| e.to_string())?.into();
    let stream = device.build_input_stream(
        &config,
        move |data: &[f32], _: &_| {
            // downmix to mono
            let mut mono = Vec::with_capacity(data.len() / channels.max(1));
            for chunk in data.chunks(channels.max(1)) {
                let avg = chunk.iter().sum::<f32>() / chunk.len() as f32;
                mono.push(avg);
            }
            let _ = samp_tx.send(mono);
        },
        |e| eprintln!("input stream error: {e}"),
        None,
    ).map_err(|e| e.to_string())?;
    stream.play().map_err(|e| e.to_string())?;

    // resample to 16k and slice into 480-sample i16 frames
    let mut resampler = SimpleResampler::new(in_rate, 16_000);
    let mut frame_acc: Vec<i16> = Vec::with_capacity(FRAME_SAMPLES_16K);
    while !stop.load(Ordering::Relaxed) {
        match samp_rx.recv_timeout(std::time::Duration::from_millis(100)) {
            Ok(mono) => {
                for s in resampler.process(&mono) {
                    let v = (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
                    frame_acc.push(v);
                    if frame_acc.len() == FRAME_SAMPLES_16K {
                        let voiced = vad.is_voice_segment(&frame_acc).unwrap_or(false);
                        if let Some(seg) = segmenter.push(&frame_acc, voiced) {
                            let _ = seg_tx.send(seg);
                        }
                        frame_acc.clear();
                    }
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
            Err(_) => break,
        }
    }
    Ok(()) // stream drops here, stopping capture
}

/// Minimal linear resampler (good enough for VAD/STT input).
struct SimpleResampler { ratio: f32, pos: f32, last: f32 }
impl SimpleResampler {
    fn new(from: usize, to: usize) -> Self { Self { ratio: from as f32 / to as f32, pos: 0.0, last: 0.0 } }
    fn process(&mut self, input: &[f32]) -> Vec<f32> {
        let mut out = Vec::new();
        for &x in input {
            while self.pos <= 1.0 {
                out.push(self.last + (x - self.last) * self.pos);
                self.pos += self.ratio;
            }
            self.pos -= 1.0;
            self.last = x;
        }
        out
    }
}
```
NOTE on external APIs: the exact `webrtc-vad` API (`Vad::new_with_rate_and_mode`, `is_voice_segment`, `SampleRate`, `VadMode`) must match the installed crate version — verify on docs.rs and adjust names if needed (the crate is small; the concept is: construct a VAD at 16 kHz, ask voiced/unvoiced per 10/20/30 ms i16 frame). Use `hound` for the wav write (add `hound = "3"` to Cargo.toml). The `SimpleResampler` is intentionally minimal (linear) — it is adequate for 16 kHz VAD/STT; `rubato` can replace it later if quality matters (keep the Cargo dep for that future swap, or drop rubato if unused — your call to keep the build warning-clean).

- [ ] **Step 2: Add commands + state**

In `src-tauri/src/state.rs`, add `pub live: std::sync::Mutex<Option<crate::translation::live::LiveSession>>` to `AppState`, initialized `None`. In `Drop`, also set the live stop flag if present:
```rust
if let Ok(mut g) = self.live.lock() {
    if let Some(sess) = g.take() { sess.stop.store(true, std::sync::atomic::Ordering::Relaxed); }
}
```
In `src-tauri/src/lib.rs`:
```rust
#[tauri::command]
fn start_live_translation(device_name: String, app: tauri::AppHandle, state: tauri::State<AppState>) -> Result<(), String> {
    let session = translation::live::start(&device_name, app)?;
    *state.live.lock().map_err(|e| e.to_string())? = Some(session);
    Ok(())
}

#[tauri::command]
fn stop_live_translation(state: tauri::State<AppState>) {
    if let Ok(mut g) = state.live.lock() {
        if let Some(sess) = g.take() {
            sess.stop.store(true, std::sync::atomic::Ordering::Relaxed);
        }
    }
}
```
Add both to `generate_handler!`. Add `pub mod live;` to `translation/mod.rs`.

- [ ] **Step 3: Verify build + tests**

Run: `cd src-tauri && cargo build` then `cargo test`.
Expected: builds clean; segmenter tests + existing tests pass. (If `webrtc-vad` API names differ, fix per docs.rs until it builds.)

- [ ] **Step 4: Commit**

```bash
git add src-tauri/src/translation/live.rs src-tauri/src/translation/mod.rs src-tauri/src/state.rs src-tauri/src/lib.rs src-tauri/Cargo.toml
git commit -m "feat(live): continuous capture + VAD segmentation + translate worker"
```

---

### Task 3: Live UI (Listen/Stop + phrase list)

**Files:**
- Modify: `index.html`
- Create: `src/live.ts`

**Interfaces:**
- Consumes: commands `start_live_translation({ deviceName })` / `stop_live_translation`; event `"phrase"` `{ source_text, translated_text, error }`.

- [ ] **Step 1: Add a Live section to index.html**

Inside `<main>`, append:
```html
<hr />
<section>
  <h2>Live translation</h2>
  <label for="live-device">Input mic:</label>
  <select id="live-device"></select>
  <button id="live-toggle">Listen</button>
  <p id="live-status">Idle</p>
  <ul id="phrases"></ul>
</section>
<script type="module" src="/src/live.ts"></script>
```

- [ ] **Step 2: Create src/live.ts**

```ts
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

const select = document.querySelector<HTMLSelectElement>("#live-device")!;
const toggle = document.querySelector<HTMLButtonElement>("#live-toggle")!;
const status = document.querySelector<HTMLParagraphElement>("#live-status")!;
const phrases = document.querySelector<HTMLUListElement>("#phrases")!;
let listening = false;

async function loadInputDevices() {
  // reuse the output-device command? we need input devices; if not present, the default mic is used.
  // For now, leave the dropdown optional: empty value = default device.
  const opt = document.createElement("option");
  opt.value = ""; opt.textContent = "Default microphone";
  select.replaceChildren(opt);
}

listen<{ source_text: string; translated_text: string; error: string | null }>("phrase", (e) => {
  const li = document.createElement("li");
  if (e.payload.error) {
    li.textContent = `⚠ ${e.payload.error}`;
  } else {
    li.textContent = `ES: ${e.payload.source_text}  →  EN: ${e.payload.translated_text}`;
  }
  phrases.appendChild(li);
});

toggle.addEventListener("click", async () => {
  try {
    if (!listening) {
      await invoke("start_live_translation", { deviceName: select.value });
      listening = true; toggle.textContent = "Stop"; status.textContent = "Listening…";
    } else {
      await invoke("stop_live_translation");
      listening = false; toggle.textContent = "Listen"; status.textContent = "Idle";
    }
  } catch (err) {
    status.textContent = `Error: ${err}`;
  }
});

loadInputDevices();
```

- [ ] **Step 3: Verify frontend build**

Run (repo root): `pnpm build` → 0 TS errors.

- [ ] **Step 4: Commit**

```bash
git add index.html src/live.ts
git commit -m "feat(ui): live translation section with phrase list"
```

---

### Task 4: Manual end-to-end verification (CPU)

**Files:** none.

- [ ] **Step 1: Run** `pnpm tauri dev`. Wait for the server to be ready (first start loads models).
- [ ] **Step 2:** Click **Listen**. Speak a Spanish phrase, pause ~1s, speak another.
- [ ] **Step 3: Confirm**
  - Each phrase appears as a list item `ES: … → EN: …` a couple seconds after you finish speaking.
  - Pausing between phrases produces **separate** list items (VAD segmentation works).
  - Speaking continuously without long pauses yields one longer segment (acceptable).
  - `nvidia-smi`: no Python on GPU (CPU-only).
  - Click **Stop** → status "Idle", no new phrases; closing the app leaves no orphan processes/threads.
- [ ] **Step 4:** Note per-phrase latency in `python/SPIKE_NOTES.md` for the Sub-phase C budget.

---

## Self-Review

**Spec coverage (against the Sub-phase B design):**
- Continuous capture + VAD + segmentation → Tasks 1 (pure) + 2 (cpal+webrtc-vad). ✅
- Separate capture/translation threads via a queue → Task 2 (producer + worker + channel). ✅
- Per-phrase translate → emit "phrase" event → UI live list → Tasks 2 + 3. ✅
- Start/Stop with clean shutdown (stop flag) → Task 2 (AtomicBool, AppState Drop) + 3. ✅
- Drop too-short/noise segments → Task 1 (min_voiced gate). ✅
- CPU-only, no cable yet (text only) → matches design (C adds the cable). ✅

**Placeholder scan:** No TBD/TODO. External-API caveats (webrtc-vad exact names) are flagged with the concrete concept to implement + where to verify, not hand-waved. Segmentation (the core, testable logic) is fully specified and unit-tested.

**Type consistency:** `Segmenter::new(silence_close, min_voiced)` / `push(&[i16], bool) -> Option<Vec<i16>>`, `FRAME_SAMPLES_16K`, `PhraseEvent { source_text, translated_text, error }` (Rust) ↔ event `"phrase"` payload ↔ JS `{ source_text, translated_text, error }`; commands `start_live_translation { deviceName }` / `stop_live_translation`; `LiveSession { stop }` consistent across tasks.

## Delivery

Branch: `feat/streaming-b-vad-capture`. On completion: PR → main (squash). Deferred items unchanged. Sub-phase C (cable playback + queue) follows.
