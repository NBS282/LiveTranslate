# LiveTranslate — Design Document

**Date:** 2026-06-16
**Status:** Approved for planning
**Scope:** MVP + phased roadmap

---

## 1. Summary

LiveTranslate is a **local-first desktop app** that turns the user into a real-time
translated speaker inside any video call. The user speaks in their source language
(Spanish, French, Portuguese, or German) and the other participant hears them **in
English, in the user's own voice**, with near-real-time latency.

All processing happens on the user's machine. Audio never leaves the device — this is
both a privacy guarantee and a deliberate product positioning toward local AI.

The app exposes its translated output as a **virtual microphone** that any call app
(Zoom, Meet, Discord) can select as its input device.

---

## 2. Goals & Non-Goals

### MVP Goals
- Real-time speech-to-speech translation from ES/FR/PT/DE **to English**.
- Preserve the speaker's voice in the translated output.
- Output routed through a virtual audio device usable by any call application.
- Cross-platform: **macOS (Apple Silicon)** and **Windows (NVIDIA GPU)**.
- Friendly onboarding: hardware check, virtual-device setup, voice sample, live test.
- Fully local inference — no audio sent to any server.

### Non-Goals (deferred to later phases)
- Language pairs other than `* → English` (Phase 2).
- Bundling the virtual audio driver with zero user friction on Windows (Phase 3).
- Bidirectional translation (hearing the other participant translated) (Phase 3).
- Explicit high-fidelity voice cloning with a persistent voice profile (Phase 2).
- Mobile (iOS/Android) clients.

---

## 3. Core Technical Decisions

### 3.1 Translation engine: Hibiki (Kyutai)
The heart of the app is **Hibiki**, an open-source (CC-BY) streaming speech-to-speech
translation model that **preserves the speaker's voice (zero-shot voice transfer)**.

Key reasons:
- Collapses STT + translation + TTS + voice transfer into a **single streaming model**,
  eliminating a fragile multi-tool pipeline.
- Streaming (chunk-by-chunk) inference enables near-real-time output instead of
  turn-based waiting.
- Voice transfer is zero-shot from the live input — **no separate cloning step needed**
  for the MVP.
- Multi-backend: PyTorch/CUDA, **MLX (Apple Silicon)**, and **Rust/Candle with
  `--features metal | cuda`**. The Rust backend aligns with the Tauri stack.

**Known constraint:** Hibiki (and the multilingual Hibiki-Zero) translate *into English*
only. Source languages: French, Spanish, Portuguese, German. This defines the MVP
language scope.

Model size options: a larger backbone for quality, a ~1B variant for lower-end hardware.

### 3.2 Application stack: Tauri (Rust + web UI)
Modeled on **Handy**'s proven architecture:
- **Rust backend** — audio capture (`cpal`), Hibiki inference (Candle), device management.
- **Web frontend** (React + TypeScript) — onboarding, controls, status.
- Cross-platform desktop packaging via Tauri.

This stack is shared by both reference projects (Handy and voicebox), reducing risk.

### 3.3 Virtual audio device (per-OS)
- **macOS:** bundle **BlackHole** (MIT license → free to redistribute). Lowest friction.
- **Windows:** **guided installation of VB-Cable** for the MVP. VB-Cable is free to use
  (donationware) and supports silent install, but **redistribution requires a separate
  license agreement with VB-Audio** — so the MVP detects/guides rather than bundles.
  Frictionless bundling on Windows is a Phase 3 concern (negotiate redistribution license
  OR ship a redistributable open-source virtual audio driver).

---

## 4. Architecture

```
┌──────────────────────────────────────────────────────────────┐
│  LiveTranslate (Tauri desktop app)                             │
│                                                                │
│   Real mic ──► Audio Capture (cpal, Rust)                      │
│                      │                                         │
│                      ▼                                         │
│              Translation Engine (Hibiki, Candle)              │
│              ES/FR/PT/DE ─► EN  + voice transfer               │
│                      │                                         │
│                      ▼                                         │
│              Virtual Output Writer                            │
│                      │                                         │
│   Web UI ◄───────────┴── status / latency / controls          │
└──────────────────────┬─────────────────────────────────────────┘
                        ▼
            Virtual Audio Device
        (BlackHole on macOS / VB-Cable on Windows)
                        ▼
        Zoom / Meet / Discord  (selects it as microphone)
                        ▼
   Remote participant hears the user in English, in the user's voice
```

---

## 5. Components

Each component is an isolated unit with a clear interface, independently testable.

| Component | Responsibility | Technology | Testable in isolation by |
|---|---|---|---|
| **Audio Capture** | Read PCM frames from the real microphone | `cpal` (Rust) | Mock input device / fixture WAV |
| **Translation Engine** | ES/FR/PT/DE → EN streaming with voice transfer | Hibiki via Candle (Metal/CUDA) | WAV-in → WAV-out golden files |
| **Virtual Output Writer** | Write translated PCM to the virtual device | OS audio APIs (Rust) | Mock output device |
| **Device Manager** | Detect, validate, and guide setup of the virtual device | Per-OS Rust modules | Mocked OS device enumeration |
| **Hardware Probe** | Detect GPU / Apple Silicon and validate requirements | Rust | Mocked capability reports |
| **Onboarding UI** | Guided first-run flow | React + TS (Tauri) | Component tests |
| **Control UI** | Source language, output device, start/stop, live status | React + TS (Tauri) | Component tests |

**Design principle:** the Translation Engine has a narrow contract — audio frames in,
translated audio frames out. The rest of the app never reaches into its internals, so
the engine (or its backend) can change without breaking consumers.

---

## 6. Data Flow & Latency

- Hibiki processes audio **chunk-by-chunk**; it accumulates just enough context to emit a
  correct translation, so output begins before the source utterance ends.
- Target latency: **near-real-time** (sub-second to a few seconds depending on hardware
  and model size). Latency is surfaced live in the UI.
- Voice transfer is applied automatically from the user's incoming audio; **there is no
  pre-call cloning step** in the MVP.

---

## 7. Onboarding Flow

Modeled on Handy's clean, low-friction first-run experience.

1. **Welcome** — one screen explaining what the app does.
2. **Hardware check** — detect GPU (NVIDIA) or Apple Silicon; warn with concrete
   requirements if unmet; never crash.
3. **Virtual device setup** — bundled BlackHole on macOS; guided VB-Cable install on
   Windows. Verify the device is present and selectable.
4. **Voice sample** — record 30–60 seconds. In the MVP this **validates** that Hibiki's
   voice transfer sounds right; the sample is also stored as the user's **voice profile**
   for Phase 2 (voicebox). The recording is never wasted.
5. **Live test** — the user speaks and hears their translated output before joining a real
   call.
6. **Ready** — pick source language and output device.

> Honesty note: the MVP does not perform explicit voice *cloning* — Hibiki transfers the
> voice live. The "cloning" framing of onboarding is fully realized in Phase 2 with
> voicebox. The voice profile is captured early so the transition is seamless.

---

## 8. Error Handling & Edge Cases

| Situation | Behavior |
|---|---|
| No compatible GPU / Intel Mac | Detected at startup; clear message with requirements; no crash |
| Virtual device missing | Guided installation flow (bundled or assisted) |
| Slow hardware / high latency | Live latency indicator; offer the smaller (~1B) model |
| Virtual device not selected in call app | In-app reminder + short instructions per call app |
| Model load failure | Surfaced as an actionable error, not a silent failure |
| Microphone permission denied (macOS) | Detected; guide the user to system settings |

No error is silently swallowed; all failures map to user-facing, actionable messages.

---

## 9. System Requirements

- **macOS:** Apple Silicon (M1 or newer). Intel Macs are not supported.
- **Windows:** dedicated NVIDIA GPU. Exact VRAM floor to be set via model benchmarks
  during implementation (validate both backbone and ~1B variant).

---

## 10. Tool Roles (clarified)

| Tool | Role | Phase |
|---|---|---|
| **Hibiki** | Core engine: X→EN streaming translation with voice transfer | MVP |
| **Handy** | Architecture/UX reference and starting patterns (Tauri, `cpal`, hotkeys, onboarding) | MVP |
| **voicebox** | Cloning + TTS engine for language pairs Hibiki does **not** cover (non-English targets), via a separate STT → text-translation → voicebox pipeline | Phase 2 |
| **PersonaPlex** | Not used — it is a full-duplex conversational agent, not a translation-pipeline component | — |

---

## 11. Phased Roadmap

- **MVP** — ES/FR/PT/DE → EN, voice transfer via Hibiki, virtual mic (BlackHole bundled /
  VB-Cable guided), onboarding, cross-platform (macOS Apple Silicon + Windows NVIDIA).
- **Phase 2** — additional language pairs (non-English targets) via a voicebox-based
  pipeline using the captured voice profile; persistent high-fidelity voice profile.
- **Phase 3** — frictionless Windows virtual-driver bundling (redistribution license or
  redistributable OSS driver); bidirectional translation (hear the other party).

---

## 12. Testing Strategy

- **Translation Engine:** golden-file tests (input WAV → expected output WAV per language),
  plus latency benchmarks per model size and backend (Metal/CUDA).
- **Audio Capture / Virtual Output:** tests against mock devices.
- **Device Manager / Hardware Probe:** unit tests with mocked OS enumeration and capability
  reports.
- **UI:** component tests for onboarding and controls.
- **End-to-end:** scripted run feeding a fixture voice through the full pipeline into a
  loopback virtual device, asserting the output is produced and routable.
- Coverage target follows project standard (80%+).
```
