# Modular Translation Engine — Design

**Date:** 2026-06-17
**Status:** Approved for planning (first iteration)
**Supersedes:** the Hibiki-Zero engine approach for the accessible build (see "Why this replaces Hibiki")
**Builds on:** Plan 1 (audio passthrough + virtual cable, merged), Plan 2a (sidecar + test UI patterns, merged)

---

## 1. Summary

Replace the heavy Hibiki-Zero S2ST model with a **modular, CPU-first translation pipeline**:
**STT → text translation → TTS**. Each stage is a small, swappable model that runs on CPU
(GPU optional), so the app runs on ordinary laptops — no NVIDIA requirement, no CUDA/nightly
stack, no GPU saturation. This directly resolves the stability crash and the cross-platform
loss that the Hibiki path caused.

The **first iteration** translates a Spanish audio **file** to an English audio file with a
**generic voice**, offline, reusing the Plan 2a sidecar + test-UI patterns. Voice cloning,
live streaming, and routing into the virtual microphone are explicit later phases (Section 8),
but the design accounts for them now so the architecture doesn't have to be reworked.

## 2. Why this replaces Hibiki (for the accessible build)

- Hibiki-Zero 3B requires an NVIDIA GPU (8GB+), needed PyTorch nightly for Blackwell, and
  **caused a full system BSOD** under load on the dev RTX 5050. It is also macOS-incompatible
  (NVIDIA-only) and translates only into English.
- A modular CPU pipeline is lighter, **stable** (stable torch CPU, no nightly), **cross-platform**
  (Windows/Mac/Linux), and — because the translation stage is decoupled — supports **any target
  language** (NLLB does 200), not just English.
- Trade-off accepted: it does **not** preserve the speaker's voice by default. Voice cloning
  returns as an optional, heavier TTS stage in a later phase (Section 8).

## 3. Architecture (first iteration — offline file→file)

```
Test UI → Tauri command (Rust) → Python sidecar (CPU, stable torch)
   input ES audio file
     → Parakeet-tdt-0.6b-v3   (STT:  audio ES → text ES)
     → NLLB-200-distilled-600M (MT:  text ES  → text EN)
     → Piper                   (TTS: text EN  → audio EN, generic voice)
   → output wav + translated text → UI plays + shows source/translated text
```

## 4. Components (sidecar Python; each isolated and replaceable)

| Component | Responsibility | Choice (first iteration) | Notes |
|---|---|---|---|
| **STT** | audio → source text | `nvidia/parakeet-tdt-0.6b-v3` (NeMo) | multilingual incl. Spanish, fast on CPU, transcribe-only |
| **MT** | source text → target text | `facebook/nllb-200-distilled-600M` | 200 languages → enables any target later; CPU-OK |
| **TTS** | target text → audio | **Piper** (ONNX, CPU; no torch) | ultralight generic voice; MeloTTS is the fallback |
| **Pipeline orchestrator** | chain STT→MT→TTS, file→file | Python entrypoint (CLI) | one narrow command, mirrors Plan 2a's sidecar contract |
| **Rust bridge** | spawn sidecar, locate wav + text | reuse Plan 2a `translate_file`/`sidecar.rs` pattern | swap executable + args; same temp-dir/output handling |
| **Test UI** | pick file, translate, play, show text | reuse Plan 2a screen | unchanged |

**Engine boundary:** the Rust side calls `translate_file(input) -> { wav, source_text, translated_text }`.
The same boundary is reused by the streaming phase (Section 8), swapping the offline orchestrator
for a streaming one — consumers don't change.

## 5. Data flow

1. User picks a Spanish audio file in the test UI.
2. Rust spawns the Python sidecar with the input path and a temp `--out-dir`.
3. Sidecar: Parakeet transcribes → NLLB translates → Piper synthesizes; writes the English wav
   (+ a text file with source and translated text) into the out-dir.
4. Rust locates the wav, reads the text, returns them; UI plays the audio and shows the text.

## 6. Error handling

- Models not downloaded → fetched from Hugging Face on first run; UI shows a "preparing models" state.
- Missing Python deps / sidecar not set up → clear, actionable error with the setup command.
- Empty/failed stage (no transcript, MT error, TTS failure) → surfaced per-stage, never a silent success.
- Slow CPU → progress/status shown; the run is long but must not freeze the UI (async command, as in Plan 2a).

## 7. Requirements & testing

- **Requirements:** CPU (any reasonable modern CPU) + a few GB RAM for the models. **No GPU required.**
  Cross-platform: Windows, macOS, Linux. Stable PyTorch CPU (Parakeet/NLLB) + ONNX (Piper).
- **Testing:** pure orchestration/parse logic unit-tested; sidecar invocation tested with a mock
  script (no real models in automated tests); manual verification with a real Spanish sample
  (CPU-only — does not risk the GPU/PC, unlike the Hibiki path).

## 8. Roadmap — how this reaches the real goal (audio in the call)

The end goal is unchanged from the original idea: **the translation must be heard by the other
person in a call, through the virtual audio cable** (built in Plan 1). The modular engine is the
middle of that chain. Phases, in risk order:

1. **This iteration** — offline file→file, generic voice, CPU. Validate the pipeline runs and
   translates correctly on modest hardware.
2. **Route output to the virtual cable** — feed the engine's output wav into the Plan 1 virtual
   device (BlackHole / VB-Cable), first file-based, so a translated clip can be "spoken" into a
   call app as the microphone. Closes the loop with Plan 1.
3. **Live streaming** — capture the mic continuously (Plan 1 `cpal`), segment by phrase (VAD),
   run STT→MT→TTS per segment, and stream the result into the virtual cable in near-real-time.
   This is where latency budgeting matters; the CPU pipeline must keep up per phrase.
4. **Voice cloning (optional, heavier)** — replace the generic TTS with a zero-shot cloning TTS
   (voicebox / XTTS-v2 / CosyVoice2) using a voice profile recorded **once** in onboarding, so the
   translated speech sounds like the user. Offered as a mode for capable hardware; generic voice
   stays as the light default.
5. **Multi-target language** — already enabled by NLLB; expose target-language selection in the UI.

> Voice cloning and the virtual-cable routing are explicitly part of the product vision; this first
> iteration deliberately ships neither, to validate the light, stable core first.
