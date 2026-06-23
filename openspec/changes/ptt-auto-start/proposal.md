# Proposal: PTT Auto-Start

## Intent

Remove the manual Start button for PTT mode. Sessions auto-start when the user selects PTT mode and registers a shortcut — the shortcut works immediately without any extra click. VAD mode stays unchanged.

## Scope

### In Scope
- Remove Start/Stop toggle from UI when in PTT mode
- Auto-call `start_live_translation_ptt` when PTT is selected and shortcut is registered
- Auto-stop session on mode switch away from PTT (back to VAD)
- Disable mode selector while session is active (prevent mid-session toggle)
- Update status text to reflect auto-started state

### Out of Scope
- Backend/Rust changes (existing commands work as-is)
- Engine or pipeline changes
- VAD mode behavior (keeps its Start button)
- Shortcut capture UX changes
- Overlay behavior changes

## Capabilities

### New Capabilities
- `ptt-auto-start`: Auto-manages PTT session lifecycle — starts on shortcut registration when PTT is active, stops on mode switch or window close

### Modified Capabilities
- None

## Approach

1. **`live.ts`**: Wire shortcut registration (`register_ptt_shortcut` success path) to auto-call `start_live_translation_ptt` if PTT mode is active and no session is running
2. **`live.ts`**: Wire mode selector — switching away from PTT while a session is active calls `stop_live_translation`
3. **`live.ts`**: Wire mode selector — switching *to* PTT with an already-registered shortcut auto-starts the session
4. **`index.html`**: Hide `#live-toggle` button when `#ptt-config` is visible (PTT mode), show it for VAD
5. **`live.ts`**: Disable mode buttons while a session is running to prevent unsafe toggles
6. **Status**: Show "PTT ready — press shortcut to record" on auto-start, "Idle" on stop

## Affected Areas

| Area | Impact | Description |
|------|--------|-------------|
| `src/live.ts` | Modified | Auto-start logic, mode-switch stop, toggle visibility wiring |
| `index.html` | Modified | Toggle visibility controlled by mode |
| `src/styles.css` | Modified | Optional: hide toggle via class when in PTT |

## Risks

| Risk | Likelihood | Mitigation |
|------|------------|------------|
| Session left running on app close | Low | Tauri window close already invokes cleanup; add `beforeunload` listener |
| Mode switch during active recording | Low | Disable mode buttons while `listening=true` |
| Re-registering shortcut mid-session | Low | Ignore re-registration if session already active, or restart gracefully |

## Rollback Plan

Revert `src/live.ts`, `index.html`, and `src/styles.css` to their pre-change state. No database or backend migration needed.

## Dependencies

None.

## Success Criteria

- [ ] PTT mode selected → no Start button visible
- [ ] Shortcut registered while in PTT → session starts automatically
- [ ] Shortcut works immediately to record without manual start
- [ ] Switching from PTT to VAD stops the session
- [ ] Switching from VAD to PTT with existing shortcut auto-starts
- [ ] VAD mode Start button works exactly as before
