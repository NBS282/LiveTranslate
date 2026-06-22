# LiveTranslate — UI/UX Redesign Design

**Date:** 2026-06-21
**Status:** Approved (pending spec review)
**Branch:** feat/windows-installer

## Goal

Replace the current AI-generated-looking UI (violet/cyan gradients, glassmorphism)
with an original, minimalist, monochrome aesthetic in the style of Handy. Simplify
the first-run setup so it hides all technical detail, rebuild onboarding as an
explanatory guided wizard, and add a user toggle to enable/disable the on-screen
subtitle overlay.

Non-goals: changing the translation engine, the audio pipeline, or any Rust backend
logic beyond what a specific UI behavior strictly requires.

## Decisions (from brainstorming)

- **Visual direction:** Monochrome "Handy" style — neutral black/grays, a single
  indigo accent used only on active state. No gradients, no glassmorphism.
- **Onboarding:** Guided step-by-step wizard on first run, plus a reopenable help
  button (`?`) on the main screen.
- **Setup:** Hide all technical output. Friendly progress text + bar + percentage.
- **Overlay toggle:** Simple on/off switch on the main screen, preference persisted.

## 1. Visual System

Single source of truth in `:root` (replaces the current violet/cyan tokens):

```
--bg        #0A0A0B   (neutral near-black)
--surface   #141416   (charcoal cards/inputs)
--surface-2 #1B1B1E   (raised / hover)
--border    #232327   (1px hairlines)
--text      #EDEDEF   (near-white)
--muted     #6B6B70   (secondary text)
--accent    #6366F1   (indigo — ACTIVE state only)
--accent-dim#4F46E5   (accent hover/pressed)
--danger    #E5484D   (stop / recording / errors)
```

Rules applied across the app:
- No `linear-gradient` backgrounds. No `backdrop-filter` glassmorphism.
- Borders are 1px hairlines using `--border`. Shadows are subtle and neutral
  (no colored glow).
- Accent color appears ONLY on: the Start button (idle), the active mode segment,
  the current wizard step indicator, the overlay toggle when ON, and focus rings.
- Stop/running state uses `--danger`, not accent.
- Typography: keep the system font stack, add `letter-spacing: -0.01em` to
  headings, increase vertical rhythm between sections.
- Logo mark `LT`: solid `--surface-2` square with `--text` monogram and a 1px
  border, replacing the gradient block.
- Border radius reduced to 10–12px for a "tool" feel rather than "landing card".
- The card entry animation and step slide-in stay (they are subtle), retuned to
  the neutral palette.

## 2. Setup Screen (first launch)

Current behavior removed from view:
- The technical checklist items (`.venv-engine`, `Piper`).
- The raw terminal log (`#setup-log`) streaming pip output.

New layout (`#screen-setup`):
- Logo + "LiveTranslate" title.
- A single friendly status line driven by the existing `setup-progress` events,
  but mapped from technical `step` strings to human phrases. Mapping table lives
  in `main.ts`:
  - download python / venv steps → "Preparando todo para ti…"
  - pip / engine package steps    → "Instalando el motor de traducción…"
  - voice model step              → "Descargando la voz…"
  - complete                      → "Casi listo…"
  - fallback (unmapped)           → "Preparando todo para ti…"
- One progress bar + a large percentage number.
- A single primary button "Comenzar instalación" that starts setup and hides
  itself once running.
- The technical log is kept in the DOM but hidden, revealed only via a discreet
  "Ver detalles" disclosure (collapsed by default) for troubleshooting failures.
- On `setup-done` failure: show a friendly error line + "Reintentar", and
  auto-expand "Ver detalles".

Backend note: `setup.rs` is NOT modified. The friendly text mapping is purely a
frontend concern reading `e.payload.step`.

## 3. Onboarding — Guided Wizard (4 steps)

`#screen-onboarding` becomes a 4-step explanatory wizard. The voice-model download
(currently onboarding step 2) moves into the silent Setup phase, so onboarding is
100% explanatory with no progress bars.

- **Step 1 · Virtual audio cable**
  What it is and why (Discord/Zoom hears translated audio as a microphone).
  `[Descargar VB-Cable]` · status badge detected/not-detected · `[Re-chequear]`.
  Continue enabled once detected.
- **Step 2 · Connect the audio** (NEW — simple text diagram)
  ```
  Tu micrófono   →  LiveTranslate
  LiveTranslate  →  "CABLE Input"   (lo elegís como Output en la app)
  "CABLE Output" →  micrófono en Discord / Zoom / Teams
  ```
- **Step 3 · Keyboard shortcuts** (NEW)
  Explains: a combo of 2+ keys (e.g. `Ctrl+Shift+Space`), used for Push-to-talk,
  and that the app must be open for the global shortcut to fire.
- **Step 4 · Done** → "Empezar a usar" → main screen.

Step indicator (dots/line) updates to 4 steps and uses the neutral palette
(accent for the current step, muted for pending, a filled neutral for done).

A `?` button in the main-screen header reopens this wizard from step 1 (reuses the
existing reopen logic, extended to reset all 4 steps).

## 4. Overlay Toggle (new feature)

On the main screen, add a switch labeled "Subtítulos en pantalla" (default ON):
- Stored in `localStorage` under key `lt.overlayEnabled` ("1"/"0").
- `live.ts` reads the preference before calling `overlay().show()`. When OFF, the
  overlay is never shown even while translating; when toggled OFF mid-session the
  overlay hides immediately; when toggled ON mid-session it shows if a session is
  active.
- The overlay (`overlay.html`) is repainted to the neutral palette: backdrop
  `rgba(10,10,11,0.82)`, 1px `--border` hairline, neutral shadow (remove the
  violet border + violet glow). The typed-cursor accent changes from cyan to
  `--accent` indigo. `src-text` muted gray, `tgt-text` near-white.

## Component Boundaries

- `styles.css` — owns the visual system; all color/spacing tokens centralized in
  `:root`. Other files reference tokens, never hardcode colors.
- `index.html` — static structure for the three screens + the overlay toggle
  control. No logic.
- `main.ts` — screen routing, friendly-text mapping, 4-step wizard control, reopen
  logic. Pure DOM + Tauri invoke; no styling.
- `live.ts` — start/stop, reads the overlay toggle preference and a small
  `setOverlayEnabled()` handler. Owns the toggle's live effect.
- `overlay.html` — self-contained styling, repainted to neutral tokens (it has its
  own inline `<style>`; keep that boundary).

## Error Handling

- Setup failure: friendly message, "Reintentar" button, auto-expand technical
  details.
- VB-Cable not found: existing error badge + download link (restyled).
- Overlay toggle: defensive read of `localStorage` (default ON if missing/invalid).

## Testing

- Manual: run `pnpm tauri dev`, walk through setup (mock or real), the 4-step
  wizard, and toggle the overlay ON/OFF mid-session verifying it shows/hides.
- Visual regression is manual (no snapshot infra in this project).
- No new automated tests required; the change is presentation + small frontend
  state. Existing Rust tests must still pass (`cargo test`).

## Out of Scope / Follow-ups

- Replacing the engine (faster-whisper migration) is tracked separately.
- Additional overlay customization (position, font size) is a possible future
  follow-up, intentionally excluded here (YAGNI).
