# Design: PTT Auto-Start

## Technical Approach

Extract shared start/stop functions from the toggle click handler into `startSession()` / `stopSession()` in `src/live.ts`. Hook those into 3 lifecycle triggers: shortcut registration success, mode switch to PTT, and mode switch away from PTT. Hide the toggle button in PTT mode using the existing `hidden` class. Add a `beforeunload` listener for cleanup on window close.

Only `src/live.ts` changes — no HTML or CSS required (the `hidden` class already exists in `styles.css`).

## Architecture Decisions

### Decision: Mode button disabled behavior

| Option | Tradeoff |
|--------|----------|
| Keep disabled for all modes while listening | Blocks PTT→VAD stop — breaks spec Req 3 |
| Remove disabled entirely | Loses safety guard for VAD mid-session switch |
| **Keep disabled only for VAD while listening** | PTT buttons stay clickable (auto-stop on switch), VAD still guarded |

**Choice**: Disable mode buttons only when `mode === "vad" && listening`. PTT mode leaves buttons enabled because the click handler handles auto-stop+switch atomically.

### Decision: Extracted start/stop vs inline duplication

**Choice**: Extract `startSession()` and `stopSession()` to avoid duplicating the invoke+error+UI logic across toggle click, shortcut registration, and mode switch paths.

**Alternatives considered**: Inlining the invoke call at each trigger point — rejected because error handling, device args, and UI updates would be repeated in 3+ places.

### Decision: Toggle visibility via JS class toggle

**Choice**: `toggle.classList.toggle("hidden", mode === "ptt")` alongside the existing `pttConfig` toggle in the mode click handler.

**Alternatives considered**: CSS-only with `.mode-selector:has([data-mode="ptt"].active) ~ #live-toggle { display: none }` — more brittle and harder to reason about. JS toggle keeps visibility coupled to mode state explicitly.

### Decision: No HTML/CSS changes needed

The existing `hidden` utility class (`.hidden { display: none !important }`) is sufficient to hide the toggle. The mode selector and PTT config HTML structure already supports the feature with JS-only changes.

## Data Flow

```
User registers shortcut
       │
       ▼
register_ptt_shortcut success
       │
       ├─ mode === "ptt" && !listening? ──→ startSession()
       │                                       │
       │                                       ▼
       │                                 invoke("start_live_translation_ptt")
       │                                       │
       │                                       ▼
       │                                 setToggle(true) → "PTT ready…"
       │
       └─ mode !== "ptt"?
             └─ Store shortcut, no auto-start

User clicks mode button
       │
       ▼
Mode click handler
       │
       ├─ PTT → VAD && listening? ──→ stopSession() → switch mode
       │
       ├─ VAD → PTT && pttShortcut? ─→ switch mode → startSession()
       │
       └─ VAD → PTT && !pttShortcut? → switch mode, "Set a PTT shortcut first"

Window close
       │
       ▼
beforeunload
       │
       └─ listening? ──→ stopSession()
```

## File Changes

| File | Action | Description |
|------|--------|-------------|
| `src/live.ts` | Modify | Extract `startSession()`/`stopSession()`, wire lifecycle hooks, hide toggle in PTT, add `beforeunload` |

## Interfaces / Contracts

No new interfaces. Existing Tauri commands reused:
```typescript
invoke("start_live_translation_ptt", { deviceName: string, outputDeviceName: string })
invoke("stop_live_translation")
invoke("register_ptt_shortcut", { shortcutStr: string })
```

`captureBtn.getAttribute("data-shortcut-registered")` guard removed — success path sets `pttShortcut` and we check that directly.

## Testing Strategy

| Layer | What to Test | Approach |
|-------|-------------|----------|
| Unit | `startSession()` edge cases (no shortcut, invoke failure) | Extract and test in isolation |
| Integration | Shortcut registration → auto-start chain | Mock Tauri invoke, verify calls |
| Integration | Mode switch PTT→VAD stop chain | Mock invoke, verify stop+mode switch |
| Integration | Mode switch VAD→PTT with/without shortcut | Mock invoke, verify start/no-start |
| E2E | Full cycle: set shortcut → auto-start → record → stop on mode switch | Manual test in Tauri dev |

## Migration / Rollout

No migration required. Toggle button will disappear from PTT mode on next app load. Existing running sessions aren't affected — the change only affects new interactions.

## Open Questions

- None
