# LiveTranslate on macOS

## Status

- **Beta.** Apple Silicon (aarch64) Macs only — Intel Macs are not supported
  yet (setup downloads an `aarch64-apple-darwin` portable Python runtime;
  there is no Intel build path).
- **Unsigned build.** macOS Gatekeeper will block the first launch with an
  "unidentified developer" warning. To open it anyway on macOS 15 (Sequoia)
  and later — the Control-click bypass was removed there:
  1. Double-click `LiveTranslate.app` once and dismiss the warning dialog.
  2. Open **System Settings → Privacy & Security**, scroll to the
     **Security** section, and click **Open Anyway** next to LiveTranslate.
  3. Confirm **Open** in the final dialog.

  On macOS 14 (Sonoma) and earlier, right-click (or Control-click) the app →
  **Open** → **Open** still works. Either way this is only needed once —
  subsequent launches work normally.

## First-run permissions

The OS will prompt for these the first time each feature is used:

- **Microphone** — required to capture your voice for live translation.
  Grant it when prompted, or under **System Settings → Privacy & Security →
  Microphone**.
- **Accessibility** or **Input Monitoring** — required for the global
  push-to-talk (PTT) shortcut to work while LiveTranslate isn't the focused
  app. Grant under **System Settings → Privacy & Security → Accessibility**
  (or **Input Monitoring**, depending on macOS version). If PTT silently
  does nothing, this permission is the first thing to check.

## System audio capture (translating what you hear, not just your mic)

macOS has no built-in loopback device, so capturing system audio (e.g. to
translate audio from a call or video) requires a virtual audio driver:

1. Install [BlackHole](https://github.com/ExistentialAudio/BlackHole)
   (2ch is sufficient): `brew install blackhole-2ch`, or download the
   installer from the project's releases page.
2. In **System Settings → Sound**, or in an app like Audio MIDI Setup,
   route the audio you want to translate to the BlackHole device.
3. In LiveTranslate's device picker, select **BlackHole 2ch** as the input
   device. LiveTranslate already recognizes BlackHole by name (see
   `src-tauri/src/audio/devices.rs`), the same way it recognizes VB-Cable
   on Windows.

## UAT checklist

Run through these in order. Check the app's log file at
`~/Library/Application Support/com.livetranslate.app/logs/engine.log` (the
Python sidecar's stderr) if anything below fails silently.

- [ ] **Setup completes** — first launch runs through all setup steps
      (Python runtime, pip packages, Piper voice, native STT model,
      translation models) without an error banner.
- [ ] **Live es→en session starts** and produces translated audio.
      Verify the native STT backend picked up Metal acceleration: launch
      the app binary from Terminal (`LiveTranslate.app/Contents/MacOS/LiveTranslate`)
      instead of double-clicking it in Finder, and look for a `GPU name:`
      line in the console output during the first transcription — that
      line only appears when `transcribe-cpp`'s Metal backend
      initializes. Its absence means it silently fell back to CPU.
- [ ] **Partial (in-progress) transcriptions** appear on screen before the
      final translation, not just at the end of a phrase.
- [ ] **Push-to-talk** — hold the configured shortcut, speak, release, and
      confirm a translation is produced. If it does nothing, check the
      Accessibility/Input Monitoring permission above.
- [ ] **Language switching** — change the source/target language pair
      mid-session and confirm the next utterance translates correctly.
- [ ] **Voice cloning** — record a voice profile, enable "use cloned
      voice", and confirm playback uses the cloned voice instead of the
      default Piper voice.
- [ ] **Piper (default voice) path** — with cloning disabled, confirm
      playback still works (regression check — cloning is optional and
      best-effort at install time).
- [ ] **RAM usage** — open Activity Monitor during a live session and note
      the memory footprint of the LiveTranslate process (and its Python
      sidecar) to flag anything that looks like a leak over a longer
      session.

## Known gaps / out of scope for this beta

- Intel Macs (no `x86_64-apple-darwin` portable Python download path).
- Code signing / notarization (hence the Gatekeeper right-click workaround
  above).
