# Walking Skeleton — Audio Passthrough Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a minimal Tauri desktop app that captures the real microphone and streams the raw (untranslated) audio into a selected virtual audio device, on macOS and Windows.

**Architecture:** A Tauri 2 app with a Rust backend doing all audio work via `cpal`. Audio flows from the default input device, through a lock-free ring buffer, into a user-selected output device (the virtual cable: BlackHole on macOS, VB-Cable on Windows). The web frontend only lists output devices and toggles start/stop. No translation yet — this validates the cross-platform audio plumbing and virtual-device routing in isolation.

**Tech Stack:** Tauri 2, Rust, `cpal` 0.15 (audio I/O), `ringbuf` 0.4 (lock-free buffer), TypeScript + minimal HTML/JS frontend, `pnpm`.

---

## Why this plan first

This is the riskiest, least-AI part of the product: getting low-level audio I/O and a virtual microphone working identically on two operating systems. If this skeleton works, plugging Hibiki into the middle (Plan 2) is a contained change. If it doesn't, nothing else matters. We prove the plumbing before touching the model.

## File Structure

```
LiveTranslate/
├── package.json                      # pnpm workspace, tauri scripts
├── src-tauri/
│   ├── Cargo.toml                    # Rust deps: tauri, cpal, ringbuf
│   ├── tauri.conf.json               # Tauri app config
│   └── src/
│       ├── main.rs                   # Tauri entrypoint, registers commands
│       ├── audio/
│       │   ├── mod.rs                # audio module exports
│       │   ├── devices.rs            # enumerate devices, find virtual output by name
│       │   └── passthrough.rs        # input->ringbuf->output stream engine
│       └── state.rs                  # holds the running stream handles
└── src/
    ├── index.html                    # device dropdown + start/stop button
    └── main.ts                       # calls Tauri commands
```

**Responsibility split:** `devices.rs` is pure-ish enumeration + selection logic (unit-testable). `passthrough.rs` owns the live streams (verified manually). `state.rs` keeps stream handles alive across Tauri commands. The frontend is intentionally dumb.

## A note on testing audio

Real audio streams need hardware and can't be asserted in a normal unit test. So we apply TDD where logic is pure — **device selection by name** and **buffer sizing** — and verify the live passthrough with an explicit, scripted manual procedure (Task 6). The manual test is a concrete checklist, not a placeholder.

---

### Task 0: Scaffold the Tauri project

**Files:**
- Create: `package.json`, `src-tauri/Cargo.toml`, `src-tauri/tauri.conf.json`, `src-tauri/src/main.rs`, `src/index.html`, `src/main.ts`

- [ ] **Step 1: Scaffold with create-tauri-app**

Run:
```bash
pnpm create tauri-app@latest . --template vanilla-ts --manager pnpm
pnpm install
```
When prompted for app name use `livetranslate`, identifier `com.livetranslate.app`.

- [ ] **Step 2: Verify the skeleton builds and runs**

Run:
```bash
pnpm tauri dev
```
Expected: a blank Tauri window opens with the default template. Close it.

- [ ] **Step 3: Commit**

```bash
git add -A
git commit -m "chore: scaffold Tauri vanilla-ts app"
```

---

### Task 1: Device selection logic (TDD, pure function)

**Files:**
- Create: `src-tauri/src/audio/mod.rs`
- Create: `src-tauri/src/audio/devices.rs`
- Modify: `src-tauri/Cargo.toml` (add `cpal`)

- [ ] **Step 1: Add cpal dependency**

In `src-tauri/Cargo.toml`, under `[dependencies]`:
```toml
cpal = "0.15"
```

- [ ] **Step 2: Write the failing test**

In `src-tauri/src/audio/devices.rs`:
```rust
/// A discoverable audio output device, identified by its display name.
#[derive(Debug, Clone, PartialEq)]
pub struct DeviceInfo {
    pub name: String,
}

/// Case-insensitive substring match of any hint against the device name.
/// Returns the first device whose name contains any of the hints.
pub fn find_virtual_output<'a>(
    devices: &'a [DeviceInfo],
    hints: &[&str],
) -> Option<&'a DeviceInfo> {
    devices.iter().find(|d| {
        let lower = d.name.to_lowercase();
        hints.iter().any(|h| lower.contains(&h.to_lowercase()))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dev(name: &str) -> DeviceInfo {
        DeviceInfo { name: name.to_string() }
    }

    #[test]
    fn finds_blackhole_on_macos() {
        let devices = vec![dev("MacBook Pro Speakers"), dev("BlackHole 2ch")];
        let found = find_virtual_output(&devices, &["blackhole", "vb-audio", "cable"]);
        assert_eq!(found, Some(&dev("BlackHole 2ch")));
    }

    #[test]
    fn finds_vbcable_on_windows() {
        let devices = vec![dev("Speakers (Realtek)"), dev("CABLE Input (VB-Audio Virtual Cable)")];
        let found = find_virtual_output(&devices, &["blackhole", "vb-audio", "cable"]);
        assert_eq!(found, Some(&dev("CABLE Input (VB-Audio Virtual Cable)")));
    }

    #[test]
    fn returns_none_when_no_virtual_device() {
        let devices = vec![dev("Speakers (Realtek)")];
        let found = find_virtual_output(&devices, &["blackhole", "vb-audio", "cable"]);
        assert_eq!(found, None);
    }
}
```

In `src-tauri/src/audio/mod.rs`:
```rust
pub mod devices;
```

In `src-tauri/src/main.rs`, add near the top (below existing attributes):
```rust
mod audio;
```

- [ ] **Step 3: Run test to verify it fails**

Run: `cd src-tauri && cargo test find_virtual_output`
Expected: compile error or FAIL until the function body above is in place. (If you pasted the body already, temporarily change the body to `None` to see a real assertion failure, then restore it.)

- [ ] **Step 4: Confirm implementation passes**

Run: `cd src-tauri && cargo test`
Expected: all three `devices::tests` pass.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/Cargo.toml src-tauri/src/audio/ src-tauri/src/main.rs
git commit -m "feat(audio): add virtual output device selection by name"
```

---

### Task 2: Enumerate real output devices + expose a Tauri command

**Files:**
- Modify: `src-tauri/src/audio/devices.rs`
- Modify: `src-tauri/src/main.rs`

- [ ] **Step 1: Add real enumeration (queries the OS, no unit test — covered manually)**

Append to `src-tauri/src/audio/devices.rs`:
```rust
use cpal::traits::{DeviceTrait, HostTrait};

/// Lists the names of all available output devices on the default host.
pub fn list_output_devices() -> Vec<DeviceInfo> {
    let host = cpal::default_host();
    match host.output_devices() {
        Ok(devices) => devices
            .filter_map(|d| d.name().ok().map(|name| DeviceInfo { name }))
            .collect(),
        Err(_) => Vec::new(),
    }
}
```

- [ ] **Step 2: Expose it as a Tauri command**

In `src-tauri/src/main.rs`, add:
```rust
#[tauri::command]
fn get_output_devices() -> Vec<String> {
    audio::devices::list_output_devices()
        .into_iter()
        .map(|d| d.name)
        .collect()
}
```
And register it in the builder:
```rust
tauri::Builder::default()
    .invoke_handler(tauri::generate_handler![get_output_devices])
    .run(tauri::generate_context!())
    .expect("error while running tauri application");
```

- [ ] **Step 3: Verify the command compiles and returns devices**

Run: `cd src-tauri && cargo build`
Expected: builds clean. Manual check happens in Task 4 via the UI.

- [ ] **Step 4: Commit**

```bash
git add src-tauri/src/
git commit -m "feat(audio): expose output device list via Tauri command"
```

---

### Task 3: Passthrough engine (input -> ring buffer -> output)

**Files:**
- Create: `src-tauri/src/audio/passthrough.rs`
- Create: `src-tauri/src/state.rs`
- Modify: `src-tauri/src/audio/mod.rs`
- Modify: `src-tauri/Cargo.toml` (add `ringbuf`)

- [ ] **Step 1: Add ringbuf dependency**

In `src-tauri/Cargo.toml`:
```toml
ringbuf = "0.4"
```

- [ ] **Step 2: Write the buffer-size helper test (TDD, pure)**

Create `src-tauri/src/audio/passthrough.rs`:
```rust
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Stream, StreamConfig};
use ringbuf::traits::{Consumer, Producer, Split};
use ringbuf::HeapRb;

/// Number of samples to buffer for a given latency in ms, sample rate, and channel count.
pub fn latency_samples(latency_ms: f32, sample_rate: u32, channels: u16) -> usize {
    let frames = (latency_ms / 1000.0) * sample_rate as f32;
    frames as usize * channels as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn computes_latency_samples() {
        // 100ms at 48kHz stereo = 0.1 * 48000 * 2 = 9600
        assert_eq!(latency_samples(100.0, 48_000, 2), 9_600);
    }

    #[test]
    fn mono_half_of_stereo() {
        assert_eq!(latency_samples(100.0, 48_000, 1), 4_800);
    }
}
```

In `src-tauri/src/audio/mod.rs`:
```rust
pub mod devices;
pub mod passthrough;
```

- [ ] **Step 3: Run the helper test to verify it fails then passes**

Run: `cd src-tauri && cargo test latency`
Expected: FAILS if function body is stubbed; PASSES with the body above. Adjust body to confirm RED first if desired.

- [ ] **Step 4: Implement the live passthrough (no unit test — manual in Task 6)**

Append to `src-tauri/src/audio/passthrough.rs`:
```rust
/// Holds the two live streams. Dropping this stops audio.
pub struct Passthrough {
    _input_stream: Stream,
    _output_stream: Stream,
}

/// Starts capturing from the default input device and writing to the
/// output device whose name matches `output_name`. Returns the live handle.
pub fn start(output_name: &str) -> Result<Passthrough, String> {
    let host = cpal::default_host();

    let input_device = host
        .default_input_device()
        .ok_or("no default input device")?;
    let output_device = host
        .output_devices()
        .map_err(|e| e.to_string())?
        .find(|d| d.name().map(|n| n == output_name).unwrap_or(false))
        .ok_or_else(|| format!("output device not found: {output_name}"))?;

    let config: StreamConfig = input_device
        .default_input_config()
        .map_err(|e| e.to_string())?
        .into();

    let buf = latency_samples(150.0, config.sample_rate.0, config.channels);
    let ring = HeapRb::<f32>::new(buf * 2);
    let (mut producer, mut consumer) = ring.split();
    // Pre-fill with silence to absorb device desync.
    for _ in 0..buf {
        let _ = producer.try_push(0.0);
    }

    let input_fn = move |data: &[f32], _: &cpal::InputCallbackInfo| {
        producer.push_slice(data);
    };
    let output_fn = move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
        let read = consumer.pop_slice(data);
        data[read..].fill(0.0);
    };
    let err_fn = |e| eprintln!("stream error: {e}");

    let input_stream = input_device
        .build_input_stream(&config, input_fn, err_fn, None)
        .map_err(|e| e.to_string())?;
    let output_stream = output_device
        .build_output_stream(&config, output_fn, err_fn, None)
        .map_err(|e| e.to_string())?;

    input_stream.play().map_err(|e| e.to_string())?;
    output_stream.play().map_err(|e| e.to_string())?;

    Ok(Passthrough {
        _input_stream: input_stream,
        _output_stream: output_stream,
    })
}
```

Create `src-tauri/src/state.rs`:
```rust
use crate::audio::passthrough::Passthrough;
use std::sync::Mutex;

/// App-wide state holding the active passthrough (if running).
#[derive(Default)]
pub struct AppState {
    pub passthrough: Mutex<Option<Passthrough>>,
}
```

- [ ] **Step 5: Verify it compiles**

Run: `cd src-tauri && cargo build`
Expected: builds clean.

- [ ] **Step 6: Commit**

```bash
git add src-tauri/Cargo.toml src-tauri/src/
git commit -m "feat(audio): add input-to-output passthrough engine"
```

---

### Task 4: Wire start/stop Tauri commands with managed state

**Files:**
- Modify: `src-tauri/src/main.rs`

- [ ] **Step 1: Register state and add start/stop commands**

In `src-tauri/src/main.rs`:
```rust
mod audio;
mod state;

use state::AppState;

#[tauri::command]
fn get_output_devices() -> Vec<String> {
    audio::devices::list_output_devices()
        .into_iter()
        .map(|d| d.name)
        .collect()
}

#[tauri::command]
fn start_passthrough(output_name: String, state: tauri::State<AppState>) -> Result<(), String> {
    let pt = audio::passthrough::start(&output_name)?;
    *state.passthrough.lock().unwrap() = Some(pt);
    Ok(())
}

#[tauri::command]
fn stop_passthrough(state: tauri::State<AppState>) {
    *state.passthrough.lock().unwrap() = None; // dropping stops the streams
}

fn main() {
    tauri::Builder::default()
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![
            get_output_devices,
            start_passthrough,
            stop_passthrough
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
```

- [ ] **Step 2: Verify it compiles**

Run: `cd src-tauri && cargo build`
Expected: builds clean.

- [ ] **Step 3: Commit**

```bash
git add src-tauri/src/main.rs
git commit -m "feat: add start/stop passthrough commands with managed state"
```

---

### Task 5: Minimal frontend (device dropdown + start/stop)

**Files:**
- Modify: `src/index.html`
- Modify: `src/main.ts`

- [ ] **Step 1: Replace the HTML body**

In `src/index.html`, set the body to:
```html
<main>
  <h1>LiveTranslate — Audio Passthrough</h1>
  <label for="device">Virtual output device:</label>
  <select id="device"></select>
  <button id="toggle">Start</button>
  <p id="status">Stopped</p>
  <script type="module" src="/main.ts"></script>
</main>
```

- [ ] **Step 2: Wire the commands**

Replace `src/main.ts` with:
```ts
import { invoke } from "@tauri-apps/api/core";

const select = document.querySelector<HTMLSelectElement>("#device")!;
const toggle = document.querySelector<HTMLButtonElement>("#toggle")!;
const status = document.querySelector<HTMLParagraphElement>("#status")!;
let running = false;

async function loadDevices() {
  const devices = await invoke<string[]>("get_output_devices");
  select.replaceChildren();
  for (const name of devices) {
    const opt = document.createElement("option");
    opt.value = name;
    opt.textContent = name;
    select.appendChild(opt);
  }
}

toggle.addEventListener("click", async () => {
  if (!running) {
    await invoke("start_passthrough", { outputName: select.value });
    running = true;
    toggle.textContent = "Stop";
    status.textContent = `Running -> ${select.value}`;
  } else {
    await invoke("stop_passthrough");
    running = false;
    toggle.textContent = "Start";
    status.textContent = "Stopped";
  }
});

loadDevices();
```

- [ ] **Step 3: Commit**

```bash
git add src/index.html src/main.ts
git commit -m "feat(ui): device dropdown and start/stop controls"
```

---

### Task 6: End-to-end manual verification

**Files:** none (verification only)

- [ ] **Step 1: Install the virtual device for your OS**

- macOS: `brew install blackhole-2ch` (or download the installer).
- Windows: download and install VB-Cable from vb-audio.com, then reboot.

- [ ] **Step 2: Run the app**

Run: `pnpm tauri dev`
Expected: window shows a dropdown populated with output devices including BlackHole / CABLE Input.

- [ ] **Step 3: Start passthrough into the virtual device**

Select the virtual device in the dropdown, click **Start**. Status shows `Running -> <device>`.

- [ ] **Step 4: Confirm audio is routed**

- macOS: open System Settings → Sound, or use a monitoring app, set input to BlackHole and listen.
- Windows: open Sound settings → Recording → "CABLE Output", enable "Listen to this device".
Speak into your mic. Expected: you hear your own voice routed through the virtual device with a small (~150ms) delay.

- [ ] **Step 5: Confirm a call app sees it**

Open Zoom/Discord audio settings. Expected: the virtual device appears as a selectable **microphone**. Select it and use the app's mic test — your voice is detected.

- [ ] **Step 6: Stop**

Click **Stop**. Status shows `Stopped`, audio routing ceases.

---

## Self-Review

**Spec coverage (against the design doc):**
- Audio Capture component → Tasks 1–3 (`cpal` input). ✅
- Virtual Output Writer → Task 3 (output to named device) + Task 6 (routing verified). ✅
- Device Manager (enumerate/select) → Tasks 1–2. ✅
- Control UI (device + start/stop) → Tasks 4–5. ✅
- Cross-platform (macOS + Windows) → Task 6 covers both; selection hints include BlackHole and VB-Cable. ✅
- Translation Engine, Hardware Probe, Onboarding → **intentionally deferred to Plans 2 and 3** (out of scope for the skeleton).

**Placeholder scan:** No TBD/TODO. Audio-stream code that can't be unit-tested is verified by the concrete manual procedure in Task 6, not hand-waved.

**Type consistency:** `DeviceInfo { name }`, `find_virtual_output`, `list_output_devices`, `latency_samples`, `Passthrough`, `start(output_name)`, `AppState.passthrough`, and the commands `get_output_devices` / `start_passthrough` / `stop_passthrough` are used consistently across backend and frontend (`outputName` in TS maps to `output_name` via Tauri's camelCase convention).

## Delivery

Branch: `feat/walking-skeleton-audio`. On completion: push, open PR to `main`, enable auto-merge (squash). CI is not yet configured (Plan 4) — until then the PR merges on manual approval.
