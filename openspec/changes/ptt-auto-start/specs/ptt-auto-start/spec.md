# ptt-auto-start Specification

## Purpose

Auto-manage the PTT session lifecycle — auto-start when ready, auto-stop on mode switch — and remove the manual Start button from PTT mode while keeping VAD behavior unchanged.

## Requirements

### Requirement: Auto-Start on Shortcut Registration

When PTT mode is active and no session is running, the system MUST call `start_live_translation_ptt` immediately after a shortcut is registered successfully.

#### Scenario: Shortcut registered while in PTT mode

- GIVEN PTT mode is active and `listening` is false
- WHEN `register_ptt_shortcut` succeeds via the Rust backend
- THEN the system MUST invoke `start_live_translation_ptt` with current device selections
- AND the UI MUST update to running state with status "PTT ready — press shortcut to record"

#### Scenario: Shortcut registered while in VAD mode

- GIVEN VAD mode is active
- WHEN a shortcut is registered
- THEN the system MUST NOT auto-start the session
- AND the shortcut MUST be stored for later use

### Requirement: Auto-Start on Mode Switch to PTT

The system MUST auto-start a PTT session when switching from VAD to PTT if a shortcut is already registered and no session is running.

#### Scenario: Switch to PTT with registered shortcut

- GIVEN a shortcut is registered and VAD mode is active (no session running)
- WHEN the user clicks the PTT mode button
- THEN the system MUST invoke `start_live_translation_ptt`
- AND the UI MUST update to running state

#### Scenario: Switch to PTT without registered shortcut

- GIVEN no shortcut is registered and VAD mode is active (no session running)
- WHEN the user clicks the PTT mode button
- THEN the system MUST NOT auto-start
- AND the status MUST show "Set a PTT shortcut first"

### Requirement: Stop on Mode Switch Away from PTT

The system MUST stop the session when switching from PTT to VAD while a session is running.

#### Scenario: Switch to VAD mid-session

- GIVEN a PTT session is running (`listening` is true)
- WHEN the user clicks the VAD mode button
- THEN the system MUST call `stop_live_translation`
- AND the session MUST be stopped
- AND the UI MUST revert to idle state

### Requirement: Toggle Hidden in PTT Mode

The Start/Stop toggle button (`#live-toggle`) MUST be hidden when PTT mode is active and visible when VAD mode is active.

#### Scenario: PTT mode hides toggle

- GIVEN PTT mode is selected
- THEN `#live-toggle` MUST have the `hidden` class

#### Scenario: VAD mode shows toggle

- GIVEN VAD mode is selected
- THEN `#live-toggle` MUST NOT have the `hidden` class

### Requirement: Mode Selector Locked During Session

Mode selector buttons MUST be disabled while a session is running to prevent unsafe mid-session toggles.

#### Scenario: Buttons disabled while listening

- GIVEN a session is running (`listening` is true)
- THEN all `.mode-btn` elements MUST be disabled
- WHEN the session stops
- THEN mode buttons MUST be re-enabled

### Requirement: Window Close Cleanup

The system SHOULD call `stop_live_translation` when the window is closed to avoid orphaned sessions.

#### Scenario: Session ends on window close

- GIVEN a PTT session is running
- WHEN the window receives a close or `beforeunload` event
- THEN the system SHOULD invoke `stop_live_translation`
