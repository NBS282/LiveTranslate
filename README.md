<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="src-tauri/icons/icon.png">
    <img src="src-tauri/icons/icon.png" width="128" alt="LiveTranslate icon — ES/EN gradient">
  </picture>
</p>

<h1 align="center">LiveTranslate</h1>

<p align="center">
  <strong>Speak in one language, be heard in another — in real time.</strong><br>
  100% local. No cloud. No API keys. No data leaves your machine.
</p>

<p align="center">
  <a href="#features">Features</a> •
  <a href="#quick-start">Quick Start</a> •
  <a href="#building-from-source">Build from Source</a> •
  <a href="#architecture">Architecture</a> •
  <a href="#contributing">Contributing</a>
</p>

<p align="center">
  <img src="https://img.shields.io/badge/license-MIT-blue" alt="MIT License">
  <img src="https://img.shields.io/badge/Windows-10%2B%20|%2011-brightgreen" alt="Windows">
  <img src="https://img.shields.io/badge/macOS-Apple%20Silicon%20(beta)-lightgrey" alt="macOS (Apple Silicon, beta)">
  <img src="https://img.shields.io/badge/Rust-♥-orange" alt="Rust">
  <img src="https://img.shields.io/badge/Python-3.12-blue" alt="Python 3.12">
</p>

---

## What is LiveTranslate?

LiveTranslate captures audio from your microphone, transcribes it with a local AI model, and translates it in real time. The translation can be shown as an on-screen subtitle overlay and/or spoken aloud by a text-to-speech engine.

The main use case is **live video calls**: you speak in your language, LiveTranslate translates and plays the audio through a virtual audio cable, and the other person on Zoom, Teams, or Meet hears you in their language — with no interpreter and no cloud service involved.

### How it works

LiveTranslate works like a simultaneous interpreter — you don't have to stop
speaking for the translation to happen:

1. **Capture** — your microphone audio is captured directly by LiveTranslate.
2. **Translate as you speak** — [Parakeet TDT 0.6B v3](https://huggingface.co/nvidia/parakeet-tdt-0.6b-v3) transcribes your speech natively in Rust (GGUF via [transcribe.cpp](https://github.com/cjpais/transcribe.cpp)) and [MarianMT](https://huggingface.co/Helsinki-NLP/opus-mt-es-en) translates it, with a dedicated model per language direction (EN ↔ ES/FR/DE). While you talk, in-progress translations stream to the subtitle overlay, and each finished sentence is detected and closed on the fly.
3. **Speak** — every closed sentence is voiced immediately — in **your own cloned voice** ([Pocket TTS](https://huggingface.co/kyutai/pocket-tts)) or a standard voice ([Piper TTS](https://github.com/rhasspy/piper)) — and played through a **virtual audio cable**: set that cable as your microphone in the video call app and the other person hears you in English, sentence by sentence, while you keep talking.

```mermaid
flowchart LR
    A["🎙️ Microphone"] --> B["Parakeet TDT 0.6B v3\nSpeech → Text\n(streaming, sentence by sentence)"]
    B --> C["MarianMT\nText → Translated Text\n(per language pair)"]
    C --> D["📺 Live Subtitle Overlay"]
    C --> E["Your cloned voice / Piper\nText → Speech"]
    E --> F["🔌 Virtual Cable\n→ Zoom / Teams / Meet"]
```

Everything runs locally on your machine. No audio or text is ever sent to the cloud.

### Virtual audio cable

To route the translated audio into a video call, you need a virtual audio cable driver:

- **Windows:** [VB-Audio Virtual Cable](https://vb-audio.com/Cable/) (free) — install it, then in your video call app select "CABLE Input" as the microphone.
- **macOS:** [BlackHole](https://existential.audio/blackhole/) (free) — install it, then select "BlackHole" as the microphone in your call app.

Without the virtual cable, LiveTranslate still works — you see the subtitles and hear the translation through your speakers, but the other party on the call won't hear the translated audio.

---

## Features

### Simultaneous translation

- **Interpreter-style flow** — sentences are detected as you speak and voiced while you continue talking; no need to pause
- **Live subtitles** — the in-progress translation streams to the overlay and refines in real time
- **Microphone mode** — translate your own speech into another language
- **English pass-through** — if you speak in English, LiveTranslate detects it and skips translation entirely; the original audio goes through unchanged
- **Push-to-talk** — hold a hotkey to translate, release to stop
- **On-screen subtitles** — transparent overlay sits above all windows, click-through, auto-hides
- **Audio playback (TTS)** — hear the translation spoken aloud (Piper voices)
- **Voice cloning** — record a short sample of your voice and have the translation spoken back in *your own* voice instead of a generic one (powered by [Pocket TTS](https://huggingface.co/kyutai/pocket-tts))

### 100% local & private

- All inference runs on your machine
- No cloud services, no API keys, no telemetry
- HuggingFace models downloaded once during setup
- Works completely offline after initial model download

### Self-contained setup

- One-click installer bundles a portable Python runtime
- Setup downloads and configures everything: Python, models, voices
- No manual Python installation, no `pip install`, no environment variables
- Survives app reinstalls (models cached in HuggingFace home directory)
- **In-app updates** — the app checks for new versions and updates itself with one click (signed releases)

### Push-to-talk (PTT) shortcut

The PTT shortcut is **user-configurable** — click the "Capture" button in settings and press any key combination (e.g. `CapsLock`, `Ctrl+Shift+Space`, `Ctrl+\`). No shortcuts are hardcoded.

In PTT mode: hold the shortcut to record, release to stop and translate.

---

## Quick Start

### Download the installer

Grab the latest installer from the [Releases page](https://github.com/NBS282/LiveTranslate/releases):

- **Windows:** `LiveTranslate-windows-x64-setup.exe`
- **macOS (Apple Silicon, beta):** `LiveTranslate-macos-aarch64.dmg` — unsigned build, see [docs/MACOS.md](docs/MACOS.md)

1. Run the installer
2. Launch LiveTranslate — the setup wizard will download Python and models automatically
3. Select your microphone
4. Choose source and target languages
5. Start speaking — subtitles appear on screen

**Requirements:**
- Windows 10/11 (64-bit) or macOS 12+ (Apple Silicon)
- ~4 GB free disk space (for Python runtime + models)
- Internet connection on first run (models download once, then fully offline)
- A virtual audio cable ([VB-Cable](https://vb-audio.com/Cable/) on Windows, [BlackHole](https://existential.audio/blackhole/) on macOS) — required to route translated audio into video calls

---

## Building from Source

### Prerequisites

- [Rust](https://rustup.rs/) (latest stable)
- [Node.js](https://nodejs.org/) 18+
- [pnpm](https://pnpm.io/)
- [Tauri CLI](https://v2.tauri.app/start/cli/)

### Steps

```bash
git clone https://github.com/NBS282/LiveTranslate.git
cd LiveTranslate

pnpm install
pnpm tauri dev      # development mode with hot-reload
pnpm tauri build    # production build
```

For detailed instructions, see [CONTRIBUTING.md](CONTRIBUTING.md).

---

## Architecture

```mermaid
graph TD
    subgraph app["Tauri Desktop App"]
        subgraph frontend["Frontend — TypeScript + Vite"]
            MW["Main Window"]
            OW["Subtitle Overlay\nalways-on-top · click-through"]
        end
        subgraph rust["Backend — Rust"]
            AUDIO["Audio Capture\ncpal (WASAPI / CoreAudio)"]
            STT["Native Speech-to-Text\nParakeet-TDT 0.6B v3 GGUF\ntranscribe.cpp — CPU / Metal"]
            SETUP["Setup Wizard"]
            ENGINE_MGR["Engine Manager"]
            CMDS["Tauri Commands & Events"]
        end
    end

    subgraph engine["AI Engine — Python + FastAPI"]
        MT["Translation\nMarianMT — one model per direction\nalternative: Canary 1B Flash (AST)"]
        TTS["Text-to-Speech\nPocket TTS (cloned voice) / Piper"]
    end

    MIC["🎙️ Microphone"] --> AUDIO
    AUDIO --> STT
    STT --> CMDS
    CMDS <-->|"Tauri events"| MW
    ENGINE_MGR -->|"spawns process"| engine
    CMDS -->|"HTTP — localhost\nfinals + streaming partials"| MT
    MT -->|"live subtitle text"| OW
    MT --> TTS
    TTS -->|"audio playback\n→ virtual cable"| MW
    SETUP -->|"first run only"| HF["HuggingFace Hub\nmodels downloaded once"]
```

### Tech Stack

| Layer | Technology |
|---|---|
| Frontend | Vanilla TypeScript, Vite |
| Desktop shell | Tauri 2 (Rust) |
| Audio capture | cpal (WASAPI on Windows, CoreAudio on macOS) |
| Speech-to-text | Parakeet TDT 0.6B v3 (multilingual ASR), native in Rust via transcribe.cpp — GGUF, CPU (tinyBLAS) / Metal on macOS |
| Translation | MarianMT (one opus-mt model per direction) |
| Alternative engine | Canary 1B Flash, speech→translated text in one pass (`LT_TRANSLATION_ENGINE=canary`, model downloads on first use) |
| Text-to-speech | Pocket TTS (voice cloning) / Piper TTS |
| Engine server | FastAPI (Python 3.12) — translation + TTS |
| Updates | Tauri updater — signed releases, one-click in-app update |

---

## Project Status

LiveTranslate is in active development. Current focus areas:

- [x] Windows installer (MSI + NSIS)
- [x] Spanish → English translation pipeline
- [x] Simultaneous translation — sentences voiced while you keep speaking (Parakeet + MarianMT)
- [x] Live streaming subtitles while you talk
- [x] Transparent subtitle overlay
- [x] Push-to-talk
- [x] Multilingual support — EN ↔ ES/FR/DE language pairs
- [x] Language selection UI
- [x] Native speech-to-text in Rust (GGUF Parakeet via transcribe.cpp) — ~3× less RAM than the Python pipeline
- [x] macOS support (Apple Silicon, beta — unsigned build)
- [x] In-app auto-updates (signed releases)
- [x] Voice cloning — instead of a generic TTS voice, the translated audio sounds like your own voice speaking the target language
- [ ] Linux support
- [ ] System audio mode — translate audio playing on your machine (videos, streams, calls you're listening to)
- [ ] Live keyboard output — type the translation as if you were physically typing it, so it appears in any text field on screen (chat, forms, live captions)

---

## FAQ

**Does it work offline?**
Yes. After the first setup downloads the models, everything runs locally with no internet connection required.

**Does it require a GPU?**
No. Speech recognition runs natively on CPU (Metal-accelerated on Apple Silicon), and the rest of the pipeline is CPU-friendly too.

**What languages are supported?**
English ↔ Spanish, French, and German, in both directions (default: Spanish → English). You pick the pair in the app.

**Is my data private?**
100%. Everything runs on your machine. No audio, text, or any data ever leaves your computer.

**Does it work with any video call app?**
Yes. LiveTranslate translates your microphone and plays the result through a virtual audio cable. You then select that cable as your microphone inside the call app. This works with any app that lets you choose an audio input — Zoom, Teams, Google Meet, Discord, and others.

**Can I use it to translate audio I'm listening to?**
Not yet — today LiveTranslate translates your microphone. System audio mode (translating videos, streams, or the other side of a call) is on the roadmap.

---

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for detailed setup and guidelines.

**Quick summary:**
1. Fork and clone
2. `pnpm install`
3. Make your changes
4. Submit a PR with conventional commits

All contributions welcome — bug fixes, features, docs, tests.

---

## License

MIT © 2026 Nicolás B. S. — see [LICENSE](LICENSE) for details.
