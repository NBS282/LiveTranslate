# Contributing to LiveTranslate

First off, thanks for taking the time to contribute! 🎉

## Code of Conduct

This project and everyone participating in it is governed by the [Code of Conduct](CODE_OF_CONDUCT.md). By participating, you are expected to uphold this code.

## How to Contribute

### Reporting Bugs

1. **Check existing issues** — make sure the bug hasn't been reported yet
2. **Use the bug report template** — include:
   - Steps to reproduce
   - Expected vs actual behavior
   - Screenshots / logs if applicable
   - System info (Windows version, GPU, etc.)

### Suggesting Features

1. Open a [feature request](https://github.com/NBS282/LiveTranslate/issues/new)
2. Describe the problem you're trying to solve, not just the solution
3. Explain why it'd be useful for most users

### Pull Requests

1. **Fork** the repo and create your branch from `main`
2. **Keep changes focused** — one feature/fix per PR
3. **Write tests** if you're adding or changing behavior
4. **Conventional commits** — we use the [Conventional Commits](https://www.conventionalcommits.org/) format:
   - `feat:` — new feature
   - `fix:` — bug fix
   - `refactor:` — code change that neither fixes nor adds
   - `docs:` — documentation only
   - `style:` — formatting, missing semicolons, etc.
   - `test:` — adding or fixing tests
   - `chore:` — build process, tooling, dependencies
5. **Keep PRs small** — under 400 lines is ideal. If it's bigger, we'll ask you to split it

## Development Setup

### Prerequisites

- [Rust](https://rustup.rs/) (latest stable)
- [Node.js](https://nodejs.org/) 18+
- [pnpm](https://pnpm.io/)
- [Tauri CLI](https://v2.tauri.app/start/cli/)

### Getting Started

```bash
# Clone your fork
git clone https://github.com/YOUR_USERNAME/LiveTranslate.git
cd LiveTranslate

# Install frontend dependencies
pnpm install

# Run in dev mode (requires a running Python engine or use mock mode)
pnpm tauri dev
```

### Project Structure

```
LiveTranslate/
├── src/                    # Frontend (Vanilla TypeScript + Vite)
│   ├── main.ts            # Main window logic
│   ├── live.ts            # Live translation stream
│   ├── overlay.ts         # Transparent subtitle overlay
│   └── styles.css         # Main window styles
├── src-tauri/              # Rust / Tauri backend
│   ├── src/
│   │   ├── lib.rs         # App setup, Tauri commands
│   │   ├── setup.rs       # One-click setup (Python, models, voices)
│   │   ├── audio/         # WASAPI loopback capture
│   │   └── translation/   # Engine server management
│   └── tauri.conf.json    # Window config, capabilities
├── python/                 # Python translation engine
│   └── lt_engine/
│       ├── pipeline.py    # ASR → MT → TTS pipeline
│       ├── server.py      # FastAPI server
│       └── server.py      # FastAPI server
├── overlay.html           # Subtitle overlay entry point
└── index.html             # Main window entry point
```

### Making Changes

1. **Frontend** (`src/`, `overlay.html`): Edit the TypeScript/CSS, Vite hot-reloads
2. **Backend** (`src-tauri/src/`): Edit Rust code, rebuild with `pnpm tauri dev`
3. **Engine** (`python/`): Edit Python, restart the engine server

## Code Style

- **Rust**: Follow `rustfmt` conventions. Run `cargo fmt` before committing
- **TypeScript**: We use the default TS config. Prefer explicit types over `any`
- **Python**: Follow PEP 8, run `ruff` if you have it
- **CSS**: BEM-like naming, custom properties for theming

## Questions?

Open a [discussion](https://github.com/NBS282/LiveTranslate/discussions) or ask in the issue tracker.
