# Streaming Sub-phase C — Cable Routing Design

**Date:** 2026-06-18
**Branch:** feat/streaming-c-cable-routing
**Status:** Approved

---

## Overview

Route the TTS-generated WAV audio from the live translation pipeline to a virtual audio cable (VB-Cable) so that external apps (Zoom, Discord, etc.) hear the translated voice in real time.

Per-phrase playback: each translated phrase plays in full before the next one starts. Voice cloning is deferred to a future phase; this phase uses the existing Piper TTS generic voice.

---

## Architecture

```
mic → VAD → segmenter → [seg_tx channel]
                               ↓
                         worker thread
                               ↓
                   engine_server::translate()
                               │
                   ┌───────────┴────────────┐
                   ▼                         ▼
             emit "phrase"          play_wav_to_device()
             event (text → UI)      → cpal output stream
                                    → VB-Cable (or default out)
```

The worker thread performs both actions sequentially per phrase: emit text event to the UI, then play the audio to the output device. A separate playback thread is not needed for this MVP; phrases are short enough that sequential processing is acceptable.

---

## Components

### `play_wav_to_device(wav_path, output_device_name)` — new fn in `translation/live.rs`

Steps:

1. `hound::WavReader::open(wav_path)` → `spec` (sample_rate, channels) + samples as `Vec<f32>`
2. Look up output device by name via cpal; fall back to system default if name is empty or not found
3. `device.default_output_config()` → `native_rate`, `native_channels`
4. If `native_rate != wav_rate` → resample using `SimpleResampler` (already in `live.rs`)
5. If `native_channels != wav_channels` → mono-to-stereo duplicate or stereo-to-mono average
6. `device.build_output_stream(native_config, callback, ...)` + `stream.play()`
7. `thread::sleep(audio_duration + 100ms buffer)`
8. Stream drops → playback stops automatically

**Expected case:** Piper outputs 22050 Hz mono. VB-Cable native is typically 44100 or 48000 Hz stereo. The resampler and channel conversion handle this exactly.

### `run_worker` — modified in `translation/live.rs`

Receives new `output_device_name: String` parameter. After emitting the `phrase` event, calls `play_wav_to_device`. Errors from playback are logged to stderr but do not abort the session — the user still sees text translations in the UI.

### `live::start()` — modified signature

```rust
pub fn start(
    device_name: &str,
    output_device_name: &str,
    app: AppHandle,
) -> Result<LiveSession, String>
```

### Tauri command `start_live_translation` — modified in `lib.rs`

```rust
#[tauri::command]
fn start_live_translation(
    device_name: String,
    output_device_name: String,
    app: tauri::AppHandle,
    state: tauri::State<AppState>,
) -> Result<(), String>
```

### Frontend — `index.html` + `src/live.ts`

- Add a second `<select id="live-output-device">` dropdown to the live section in `index.html`
- In `live.ts`, populate it on load using the existing `get_output_devices()` Tauri command (already used by the passthrough section)
- Pass `outputDeviceSelect.value` as `outputDeviceName` when invoking `start_live_translation`

---

## Data Flow

```
TranslationOutput {
    output_wav: PathBuf,   // temp WAV written by Piper, 22050 Hz mono
    source_text: String,
    translated_text: String,
}
    │
    ├─► app.emit("phrase", PhraseEvent { source_text, translated_text })
    │
    └─► play_wav_to_device(&output_wav, &output_device_name)
            │
            ├─ hound reads samples
            ├─ SimpleResampler: 22050 → native_rate
            ├─ channel expand: mono → stereo (duplicate)
            └─ cpal output stream → VB-Cable
```

The `output_wav` file is deleted **after** playback completes (moved from before playback in the current code).

---

## Error Handling

| Failure | Behavior |
|---------|----------|
| Output device not found | Log to stderr, skip playback, continue session |
| cpal stream build error | Log to stderr, skip playback, continue session |
| hound read error | Log to stderr, skip playback, continue session |
| Playback errors are non-fatal | Text still appears in UI |

If `output_device_name` is empty, `play_wav_to_device` uses the system default output device.

---

## Testing

- **Unit test:** `play_wav_to_device` with a nonexistent device name returns `Err` without panicking
- **Unit test:** channel conversion — mono samples duplicated correctly to stereo
- **Existing tests:** all 15 pass unchanged
- **Manual:** `pnpm tauri dev` → select VB-Cable as output → speak Spanish → translated voice heard in VB-Cable monitor or Zoom

---

## Out of Scope

- Voice cloning (future phase)
- Streaming TTS (playback while generating)
- Playback queue / overlap handling
- Volume control
