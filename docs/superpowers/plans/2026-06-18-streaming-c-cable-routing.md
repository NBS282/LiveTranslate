# Streaming Sub-phase C — Cable Routing Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Play each TTS-generated phrase WAV to a user-selected cpal output device (VB-Cable) immediately after translation, so external apps hear the translated voice.

**Architecture:** The worker thread in `live.rs` already translates each phrase and emits a text event. We add a `play_wav_to_device()` call after the emit. A new output device dropdown in the UI passes the device name through the existing Tauri command.

**Tech Stack:** Rust, cpal (already dep), hound (already dep), Tauri 2, TypeScript

## Global Constraints

- Piper TTS outputs 16-bit integer mono WAV at 22050 Hz — resampling to the device's native rate is required
- VB-Cable typically presents as 44100 or 48000 Hz stereo — mono-to-stereo channel conversion is required
- Playback errors must NOT abort the live session — log to stderr and continue
- All existing 15 tests must continue to pass
- No new Rust dependencies — use hound and cpal already in Cargo.toml

---

### Task 1: `convert_channels` + `play_wav_to_device` with tests

**Files:**
- Modify: `src-tauri/src/translation/live.rs`

**Interfaces:**
- Produces:
  - `fn convert_channels(samples: &[f32], from_ch: usize, to_ch: usize) -> Vec<f32>`
  - `fn play_wav_to_device(wav_path: &std::path::Path, output_device_name: &str) -> Result<(), String>`

- [ ] **Step 1: Write the failing tests**

Add inside the existing `#[cfg(test)]` block at the bottom of `src-tauri/src/translation/live.rs` (or create one if absent):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn mono_to_stereo_duplicates_each_sample() {
        let mono = vec![0.5f32, -0.3, 0.1];
        let stereo = convert_channels(&mono, 1, 2);
        assert_eq!(stereo, vec![0.5, 0.5, -0.3, -0.3, 0.1, 0.1]);
    }

    #[test]
    fn stereo_to_mono_averages_pairs() {
        let stereo = vec![0.6f32, -0.6, 0.2, 0.4];
        let mono = convert_channels(&stereo, 2, 1);
        assert!((mono[0] - 0.0).abs() < 1e-5, "expected 0.0, got {}", mono[0]);
        assert!((mono[1] - 0.3).abs() < 1e-5, "expected 0.3, got {}", mono[1]);
    }

    #[test]
    fn same_channel_count_is_passthrough() {
        let samples = vec![0.1f32, 0.2, 0.3];
        assert_eq!(convert_channels(&samples, 1, 1), samples);
    }

    #[test]
    fn play_nonexistent_wav_returns_err() {
        let result = play_wav_to_device(Path::new("does_not_exist_xyz.wav"), "");
        assert!(result.is_err(), "expected Err for missing WAV file");
    }
}
```

- [ ] **Step 2: Run tests — expect FAIL (functions not yet defined)**

```
cd src-tauri
cargo test translation::live::tests
```

Expected: compile error — `convert_channels` and `play_wav_to_device` not found.

- [ ] **Step 3: Add `convert_channels` to `live.rs`**

Insert before the existing `struct SimpleResampler` declaration:

```rust
/// Convert between channel counts. Supports mono↔stereo; passes through otherwise.
fn convert_channels(samples: &[f32], from_ch: usize, to_ch: usize) -> Vec<f32> {
    match (from_ch, to_ch) {
        (1, 2) => samples.iter().flat_map(|&s| [s, s]).collect(),
        (2, 1) => samples
            .chunks(2)
            .map(|c| (c[0] + c.get(1).copied().unwrap_or(0.0)) / 2.0)
            .collect(),
        _ => samples.to_vec(),
    }
}
```

- [ ] **Step 4: Add `play_wav_to_device` to `live.rs`**

Insert after `convert_channels`:

```rust
/// Read a WAV file and play it to the named cpal output device.
/// Falls back to the system default if `output_device_name` is empty or not found.
/// Returns Err on any setup failure; playback errors are logged but non-fatal.
fn play_wav_to_device(wav_path: &std::path::Path, output_device_name: &str) -> Result<(), String> {
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

    // --- Read WAV ---
    let mut reader = hound::WavReader::open(wav_path).map_err(|e| e.to_string())?;
    let spec = reader.spec();
    let wav_rate = spec.sample_rate as usize;
    let wav_channels = spec.channels as usize;

    let samples_f32: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => {
            reader.samples::<f32>().filter_map(|s| s.ok()).collect()
        }
        hound::SampleFormat::Int => {
            let max = (1i64 << (spec.bits_per_sample.saturating_sub(1))) as f32;
            reader
                .samples::<i32>()
                .filter_map(|s| s.ok())
                .map(|s| s as f32 / max)
                .collect()
        }
    };

    // --- Find device ---
    let host = cpal::default_host();
    let device = if output_device_name.is_empty() {
        host.default_output_device()
    } else {
        host.output_devices()
            .map_err(|e| e.to_string())?
            .find(|d| d.name().map(|n| n == output_device_name).unwrap_or(false))
            .or_else(|| host.default_output_device())
    }
    .ok_or_else(|| "no output device available".to_string())?;

    let native_cfg = device.default_output_config().map_err(|e| e.to_string())?;
    let native_rate = native_cfg.sample_rate().0 as usize;
    let native_channels = native_cfg.channels() as usize;

    // --- Resample ---
    let resampled = if native_rate != wav_rate {
        let mut r = SimpleResampler::new(wav_rate, native_rate);
        r.process(&samples_f32)
    } else {
        samples_f32
    };

    // --- Channel conversion ---
    let final_samples = convert_channels(&resampled, wav_channels, native_channels);
    let duration_secs = final_samples.len() as f32 / native_rate as f32 / native_channels as f32;

    // --- Build and play stream ---
    let samples = std::sync::Arc::new(std::sync::Mutex::new(final_samples.into_iter()));
    let samples_cb = std::sync::Arc::clone(&samples);

    let stream_cfg = cpal::StreamConfig {
        channels: native_channels as u16,
        sample_rate: cpal::SampleRate(native_rate as u32),
        buffer_size: cpal::BufferSize::Default,
    };

    let stream = device
        .build_output_stream(
            &stream_cfg,
            move |data: &mut [f32], _| {
                let mut iter = samples_cb.lock().unwrap();
                for d in data.iter_mut() {
                    *d = iter.next().unwrap_or(0.0);
                }
            },
            |e| eprintln!("playback stream error: {e}"),
            None,
        )
        .map_err(|e| e.to_string())?;

    stream.play().map_err(|e| e.to_string())?;
    std::thread::sleep(std::time::Duration::from_secs_f32(duration_secs + 0.1));

    Ok(())
}
```

- [ ] **Step 5: Run tests — expect PASS**

```
cd src-tauri
cargo test translation::live::tests
```

Expected: all 4 new tests + existing 15 pass. Total 19.

- [ ] **Step 6: Commit**

```
git add src-tauri/src/translation/live.rs
git commit -m "feat(live): add play_wav_to_device + convert_channels with tests"
```

---

### Task 2: Wire output device into worker thread + fix temp dir cleanup

**Files:**
- Modify: `src-tauri/src/translation/live.rs`

**Interfaces:**
- Consumes: `play_wav_to_device` (Task 1), `convert_channels` (Task 1)
- Produces:
  - `pub fn start(device_name: &str, output_device_name: &str, app: AppHandle) -> Result<LiveSession, String>`
  - `run_worker` with signature `fn run_worker(rx: Receiver<Vec<i16>>, app: AppHandle, stop: Arc<AtomicBool>, output_device: String)`

- [ ] **Step 1: Update `run_worker` signature and body**

Replace the existing `run_worker` function in `live.rs`:

```rust
fn run_worker(rx: Receiver<Vec<i16>>, app: AppHandle, stop: Arc<AtomicBool>, output_device: String) {
    while !stop.load(Ordering::Relaxed) {
        match rx.recv_timeout(std::time::Duration::from_millis(250)) {
            Ok(samples) => {
                if samples.len() < 8_000 {
                    continue;
                }
                let translate_result = write_segment_wav(&samples).and_then(|p| {
                    let result = crate::translation::engine_server::translate(&p);
                    let _ = std::fs::remove_file(&p);
                    result
                });

                let evt = match &translate_result {
                    Ok(out) => PhraseEvent {
                        source_text: out.source_text.clone(),
                        translated_text: out.translated_text.clone(),
                        error: None,
                    },
                    Err(e) => PhraseEvent {
                        source_text: String::new(),
                        translated_text: String::new(),
                        error: Some(e.clone()),
                    },
                };
                let _ = app.emit("phrase", evt);

                if let Ok(ref out) = translate_result {
                    if let Err(e) = play_wav_to_device(&out.output_wav, &output_device) {
                        eprintln!("playback error: {e}");
                    }
                    // Clean up output WAV and its temp parent dir.
                    let _ = std::fs::remove_file(&out.output_wav);
                    if let Some(dir) = out.output_wav.parent() {
                        let _ = std::fs::remove_dir(dir);
                    }
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
            Err(_) => break,
        }
    }
}
```

- [ ] **Step 2: Update `live::start()` signature and worker spawn**

Replace the `start` function signature and the worker thread spawn inside it:

```rust
pub fn start(device_name: &str, output_device_name: &str, app: AppHandle) -> Result<LiveSession, String> {
    let host = cpal::default_host();
    let device = host
        .input_devices()
        .map_err(|e| e.to_string())?
        .find(|d| d.name().map(|n| n == device_name).unwrap_or(false))
        .or_else(|| host.default_input_device())
        .ok_or_else(|| "no input device available".to_string())?;

    let default_config = device.default_input_config().map_err(|e| e.to_string())?;
    let in_rate = default_config.sample_rate().0 as usize;
    let channels = default_config.channels() as usize;

    let (seg_tx, seg_rx): (Sender<Vec<i16>>, Receiver<Vec<i16>>) = std::sync::mpsc::channel();
    let stop = Arc::new(AtomicBool::new(false));

    // Worker thread: translate segments, emit events, play audio.
    {
        let app_clone = app.clone();
        let stop_clone = stop.clone();
        let output_device = output_device_name.to_string();
        std::thread::spawn(move || run_worker(seg_rx, app_clone, stop_clone, output_device));
    }

    // Producer thread: capture → resample → VAD → segment → send.
    let stop_prod = stop.clone();
    std::thread::spawn(move || {
        if let Err(e) = run_producer(device, in_rate, channels, seg_tx, stop_prod) {
            eprintln!("live producer error: {e}");
        }
    });

    Ok(LiveSession { stop })
}
```

- [ ] **Step 3: Run all tests**

```
cd src-tauri
cargo test
```

Expected: 19 tests pass, 0 fail.

- [ ] **Step 4: Commit**

```
git add src-tauri/src/translation/live.rs
git commit -m "feat(live): route TTS audio to output device, fix temp dir cleanup"
```

---

### Task 3: Update `start_live_translation` Tauri command

**Files:**
- Modify: `src-tauri/src/lib.rs`

**Interfaces:**
- Consumes: `translation::live::start(&str, &str, AppHandle)` (Task 2)
- Produces: `start_live_translation(device_name, output_device_name, app, state)` Tauri command

- [ ] **Step 1: Replace `start_live_translation` in `lib.rs`**

Find and replace the existing `start_live_translation` function:

```rust
#[tauri::command]
fn start_live_translation(
    device_name: String,
    output_device_name: String,
    app: tauri::AppHandle,
    state: tauri::State<AppState>,
) -> Result<(), String> {
    let session = translation::live::start(&device_name, &output_device_name, app)?;
    *state.live.lock().map_err(|e| e.to_string())? = Some(session);
    Ok(())
}
```

`stop_live_translation` is unchanged.

- [ ] **Step 2: Run all tests**

```
cd src-tauri
cargo test
```

Expected: 19 tests pass, 0 fail, no compile errors.

- [ ] **Step 3: Commit**

```
git add src-tauri/src/lib.rs
git commit -m "feat(live): add output_device_name param to start_live_translation command"
```

---

### Task 4: Frontend — output device dropdown

**Files:**
- Modify: `index.html`
- Modify: `src/live.ts`

**Interfaces:**
- Consumes: `get_output_devices()` Tauri command (already exists, already used by passthrough section)
- Consumes: `start_live_translation({ deviceName, outputDeviceName })` Tauri command (Task 3)

- [ ] **Step 1: Add output device dropdown to `index.html`**

In the live translation `<section>`, add a label and select for the output device after the existing input mic select:

```html
<section>
  <h2>Live translation</h2>
  <label for="live-device">Input mic:</label>
  <select id="live-device"></select>
  <label for="live-output-device">Output (cable):</label>
  <select id="live-output-device"></select>
  <button id="live-toggle">Listen</button>
  <p id="live-status">Idle</p>
  <ul id="phrases"></ul>
</section>
```

- [ ] **Step 2: Update `src/live.ts`**

Replace the entire file content:

```typescript
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

const inputSelect = document.querySelector<HTMLSelectElement>("#live-device")!;
const outputSelect = document.querySelector<HTMLSelectElement>("#live-output-device")!;
const toggle = document.querySelector<HTMLButtonElement>("#live-toggle")!;
const status = document.querySelector<HTMLParagraphElement>("#live-status")!;
const phrases = document.querySelector<HTMLUListElement>("#phrases")!;
let listening = false;

async function loadDevices() {
  const opt = document.createElement("option");
  opt.value = "";
  opt.textContent = "Default microphone";
  inputSelect.replaceChildren(opt);

  const outputs: string[] = await invoke("get_output_devices");
  outputSelect.replaceChildren();
  for (const name of outputs) {
    const o = document.createElement("option");
    o.value = name;
    o.textContent = name;
    outputSelect.appendChild(o);
  }
}

listen<{ source_text: string; translated_text: string; error: string | null }>(
  "phrase",
  (e) => {
    const li = document.createElement("li");
    if (e.payload.error) {
      li.textContent = `⚠ ${e.payload.error}`;
    } else {
      li.textContent = `ES: ${e.payload.source_text}  →  EN: ${e.payload.translated_text}`;
    }
    phrases.appendChild(li);
  }
);

toggle.addEventListener("click", async () => {
  try {
    if (!listening) {
      await invoke("start_live_translation", {
        deviceName: inputSelect.value,
        outputDeviceName: outputSelect.value,
      });
      listening = true;
      toggle.textContent = "Stop";
      status.textContent = "Listening…";
    } else {
      await invoke("stop_live_translation");
      listening = false;
      toggle.textContent = "Listen";
      status.textContent = "Idle";
    }
  } catch (err) {
    status.textContent = `Error: ${err}`;
  }
});

loadDevices();
```

- [ ] **Step 3: Build and smoke-test**

```
pnpm tauri dev
```

Expected:
- App opens, Live section shows two dropdowns: "Input mic" (default mic) and "Output (cable)" (list of output devices including VB-Cable)
- Select VB-Cable in "Output (cable)"
- Click "Listen", speak Spanish
- Phrases appear in the UI list
- Translated English voice plays through VB-Cable

- [ ] **Step 4: Commit**

```
git add index.html src/live.ts
git commit -m "feat(ui): add output device selector to live translation section"
```

---

## Self-Review

**Spec coverage:**
- ✅ `play_wav_to_device` with hound + cpal — Task 1
- ✅ Resample 22050→native rate — Task 1 (SimpleResampler)
- ✅ Mono→stereo channel conversion — Task 1 (`convert_channels`)
- ✅ Output device fallback to default when empty/not found — Task 1
- ✅ Playback errors non-fatal, logged to stderr — Task 2
- ✅ WAV deleted after playback (not before) — Task 2
- ✅ Temp output dir cleaned up — Task 2 (fix for existing bug)
- ✅ `live::start()` signature updated — Task 2
- ✅ Tauri command updated — Task 3
- ✅ Frontend dropdown + invoke update — Task 4
- ✅ Unit tests: channel conversion + missing WAV error — Task 1
- ✅ All 15 existing tests unchanged — verified in each task

**Placeholder scan:** None found.

**Type consistency:**
- `play_wav_to_device(&Path, &str) -> Result<(), String>` — defined Task 1, consumed Task 2 ✅
- `convert_channels(&[f32], usize, usize) -> Vec<f32>` — defined Task 1, consumed Task 1 internally ✅
- `live::start(&str, &str, AppHandle)` — defined Task 2, consumed Task 3 ✅
- `start_live_translation({ deviceName, outputDeviceName })` — defined Task 3, consumed Task 4 ✅
