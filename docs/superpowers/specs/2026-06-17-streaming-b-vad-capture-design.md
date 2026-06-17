# Streaming Sub-phase B — VAD + Continuous Capture Design

**Date:** 2026-06-17
**Status:** Approved for planning
**Part of:** Live streaming, decomposed into A (persistent server, DONE), **B (VAD + continuous capture)**, C (route to virtual cable + queue).
**Builds on:** Sub-phase A (persistent FastAPI server, merged), Plan 1 (`cpal` mic capture, merged).

---

## 1. Summary

Add a **"live listen" mode**: continuously capture the microphone, segment the user's speech
into phrases with a voice-activity detector (VAD), send each phrase to the persistent server's
`/translate`, and show the source/translated text **phrase-by-phrase, live, in the UI**.

Sub-phase B does NOT yet route audio to the virtual cable (that is C). It validates the live
chain (mic → VAD → translation) by surfacing each translated phrase as text. The translated
`output.wav` is produced by the server but not played in B.

## 2. Goals & Non-Goals

### Goals
- Continuous mic capture (reuse Plan 1 `cpal`), VAD segmentation (`webrtc-vad`).
- Each detected phrase → `/translate` → emit a Tauri event → UI appends ES/EN text live.
- Capture and translation on **separate threads** (a queue) so a ~2s translation never drops capture.
- CPU-only, cross-platform, no GPU.

### Non-Goals (Sub-phase C / later)
- Playing translated audio to the virtual cable + playback queue/ordering.
- Barge-in / overlapping-speech handling, partial/streaming transcripts.
- Voice cloning, multi-target language UI.

## 3. Architecture

```
UI [Listen]  → start_live_translation(device_name)
Rust live.rs:
  Producer thread (cpal input stream on the chosen mic):
    raw frames → resample to 16 kHz mono i16 → webrtc-vad per 30ms frame
    accumulate while voiced; on ~700ms trailing silence → close segment
    (discard segments < ~300ms) → push segment to a channel (queue)
  Worker thread:
    recv segment → write 16 kHz wav to temp → engine_server::translate(path)
    → app.emit("phrase", { source_text, translated_text })
UI: on "phrase" event → append "ES: … / EN: …" to a live list
UI [Stop]    → stop_live_translation  (signals producer+worker to end, joins them)
```

## 4. Components (isolated)

| Component | Responsibility | Notes |
|---|---|---|
| **`translation/live.rs`** (new) | Producer (capture+resample+VAD+segment) + worker (translate+emit); start/stop | Reuses `engine_server::translate` from Sub-phase A |
| **VAD + resample** | `webrtc-vad` (16 kHz, i16, 30ms frames) + `rubato` (device rate → 16 kHz) | Parakeet is also 16 kHz, so segments suit STT directly |
| **Segmentation logic** (pure) | Given per-frame voiced flags, decide segment boundaries (start on voice, end on N ms silence, drop too-short) | Unit-tested without audio hardware |
| **Tauri commands** | `start_live_translation(device)`, `stop_live_translation`; event `"phrase"` | |
| **AppState** (modify) | Hold live-mode handles + a stop flag (`AtomicBool`) to end threads cleanly | Alongside the existing audio thread + server child |
| **UI** | "Live" section: device pick (reuse existing dropdown) + Listen/Stop + scrolling phrase list | Listens to the `"phrase"` event |

**Boundary:** `live.rs` depends only on `engine_server::translate` (input path → result) and the
Tauri `AppHandle` (to emit events). Segmentation is a pure function consuming a stream of
voiced/unvoiced flags, so it's testable in isolation.

## 5. Data flow

1. User clicks Listen → `start_live_translation(device)` opens a `cpal` input stream on that mic.
2. Each input callback: resample to 16 kHz mono, feed 30ms i16 frames to `webrtc-vad`.
3. Segmentation: enter "in speech" on voiced frames; after ~700ms of silence, emit the accumulated
   segment (if ≥ ~300ms) to the queue and reset.
4. Worker: write the segment to a temp 16 kHz wav, call `engine_server::translate`, emit `"phrase"`.
5. UI appends the ES/EN texts. Stop signals threads to finish and joins them.

## 6. Error handling

| Situation | Behavior |
|---|---|
| Server not ready when a phrase fires | `translate` returns Err → emit a `"phrase"` with an error note (don't crash the loop) |
| Chosen mic unavailable / stream build fails | `start_live_translation` returns Err → UI shows it; no threads left running |
| Stop requested mid-translation | Stop flag set; producer ends; worker finishes the in-flight phrase then exits (bounded) |
| Very short / noise-only segments | Dropped by the < ~300ms guard before reaching the queue |

## 7. Requirements & testing

- **Deps:** Rust adds `webrtc-vad` + `rubato`. Reuses Sub-phase A server + `reqwest`. CPU-only.
- **Testing:**
  - Pure **segmentation** logic: feed sequences of voiced/unvoiced flags → assert segment boundaries, silence-close, and short-segment drop. Unit-tested, no hardware.
  - Manual: click Listen, speak a few Spanish phrases, confirm each appears translated (ES/EN) live; pausing between phrases produces separate segments; Stop ends cleanly with no orphan threads.

## 8. Relationship to C

B yields a stream of translated phrases (text + an `output.wav` per phrase from the server).
**Sub-phase C** takes those `output.wav`s and plays them through the Plan 1 virtual cable with a
**queue** so phrases don't overlap, completing the "natural in a call" loop. If per-phrase latency
(~2s + VAD trailing silence) feels too high, that's where we revisit chunking/streaming transport.
