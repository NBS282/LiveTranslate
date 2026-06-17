# Streaming Sub-phase A — Persistent Translation Server Design

**Date:** 2026-06-17
**Status:** Approved for planning
**Part of:** Live streaming (the "natural in a call" goal), decomposed into A (persistent server), B (VAD + continuous capture), C (route to virtual cable + sync). This doc is **A only**.
**Builds on:** Modular CPU engine (`lt_engine`, merged), Plan 1 (audio + virtual cable, merged).

---

## 1. Summary

Replace the per-file engine invocation (`python -m lt_engine`, which reloads all models ~20s
every call) with a **persistent FastAPI server** that loads Parakeet + NLLB + Piper **once** at
startup and translates over HTTP. The Tauri/Rust app launches it as a sidecar, waits for
readiness, and sends translation requests to `127.0.0.1`. Steady-state per request drops from
~20s (reload) to ~2s (inference only).

This is the foundation of live streaming: without models held in memory, nothing is viable in
real time. Sub-phase A does NOT add microphone capture, VAD, or cable routing — those are B and
C. The existing file→file test UI stays, but now talks to the server.

## 2. Goals & Non-Goals

### Goals
- Persistent Python server: models loaded once, translate via HTTP.
- Rust launches it as a managed sidecar, health-checks readiness, and shuts it down on exit.
- Steady-state translation ~2s (no per-call reload), proven by translating several files in a row.
- Keep CPU-only, cross-platform, no GPU.

### Non-Goals (later sub-phases / phases)
- Microphone capture, VAD, segmentation (Sub-phase B).
- Routing translated audio to the virtual cable + playback queue (Sub-phase C).
- WebSocket / continuous audio streaming (only if per-segment HTTP latency proves insufficient in C).
- Voice cloning, multi-target-language UI.

## 3. Architecture

```
App Tauri (Rust)
  • on startup → spawn sidecar:  <engine-python> -m lt_engine.server   (binds 127.0.0.1:8765)
  • wait        → GET /health until { ready: true } (retry with timeout)
  • translate   → write input wav to a temp dir → POST /translate { input_path, out_dir }
                                                 ← { output_wav, source_text, translated_text }
  • on exit     → kill the sidecar process
Server (lt_engine/server.py, FastAPI + uvicorn)
  • startup     → load Parakeet + NLLB + Piper ONCE (module singletons)
  • GET  /health    → { "ready": true } once models are loaded
  • POST /translate → { input_path, out_dir, src?, tgt? } → run STT→MT→TTS with the loaded models
                      → write output.wav + return { output_wav, source_text, translated_text }
```

## 4. Components (isolated, well-bounded)

| Component | Responsibility | Notes |
|---|---|---|
| **`lt_engine/pipeline.py`** (refactor) | Hold the 3 models as lazy singletons; `transcribe/translate/synthesize` reuse them | Today they reload per call — change to load-once. CLI `__main__` still works (one load per process). |
| **`lt_engine/server.py`** (new) | FastAPI app: `/health`, `/translate`; trigger model load at startup | Binds 127.0.0.1 only. Uses pipeline singletons. |
| **`src-tauri/src/translation/engine_server.rs`** (new) | Spawn server sidecar, health-check w/ retry, HTTP client for `/translate`, hold child handle for shutdown | Replaces the per-file `Command` spawn in `translate_file`. |
| **`translate_file` (sidecar.rs)** (modify) | Now: write temp wav → HTTP POST to the server → parse response | Same `TranslationOutput` contract; consumers unchanged. |
| **AppState (state.rs)** (modify) | Hold the server child handle so it's killed on app exit | Mirrors the existing managed-state pattern. |

**Boundary preserved:** `translate_file(&Path) -> Result<TranslationOutput, String>` keeps the
same signature; only its internals change (HTTP instead of subprocess). The Tauri command and UI
are untouched. Sub-phases B/C reuse this server (B sends segments to it; C plays its output).

## 5. Data flow

1. App starts → Rust spawns the server, polls `GET /health` until ready (bounded retry, e.g. 60s).
2. UI translate → Rust writes the input wav to a temp `out_dir`, `POST /translate { input_path, out_dir }`.
3. Server runs STT→MT→TTS with the in-memory models, writes `output.wav` + returns the texts as JSON.
4. Rust reads `output.wav` path + texts from the response, returns `TranslationOutput`; UI plays + shows text.
5. App exit → Rust kills the server process.

## 6. Error handling

| Situation | Behavior |
|---|---|
| Server fails to start / never becomes ready | Health-check times out → clear error ("translation server failed to start") |
| Server crashes mid-session | HTTP request errors → surfaced; (auto-restart is a later refinement, not now) |
| Translation error (empty transcript, model failure) | Server returns HTTP 4xx/5xx with a message → Rust maps to a UI error |
| Port already in use | Configurable port via env (`LT_ENGINE_PORT`, default 8765); startup failure surfaced |

## 7. Security

- Server binds **`127.0.0.1` only** (never `0.0.0.0`) — not reachable from the network.
- No auth (local-only IPC). Input path validated (extension allowlist + exists) as today.
- No untrusted input reaches a shell; paths passed as JSON, used with safe file APIs.

## 8. Requirements & testing

- **Deps:** Python adds `fastapi` + `uvicorn`; Rust adds `reqwest` (blocking client is fine inside the existing `spawn_blocking` command).
- **CPU-only, cross-platform** unchanged.
- **Testing:**
  - Server: FastAPI `TestClient` with the model functions monkeypatched/mocked → assert `/health` shape and `/translate` request/response contract (no real models in automated tests).
  - Rust: pure parsing of the `/translate` JSON response (unit-tested), and health-check retry logic where pure.
  - **Manual:** launch the app, translate 2-3 Spanish files in a row; confirm the first includes model-load time but subsequent ones are ~2s (proves persistence). Still CPU — no GPU risk.

## 9. Relationship to B and C

A gives a warm, always-ready translation service. **B** adds mic capture + VAD to produce
segments and sends each to `/translate`. **C** plays each returned `output.wav` through the Plan 1
virtual cable with a queue so phrases don't overlap. If per-segment HTTP latency is too high in C,
revisit a streaming transport (WebSocket) — but only then.
