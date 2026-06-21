# Delta for live-translate

## ADDED Requirements

### Requirement: macOS ASR Path in Worker

On macOS, the live worker MUST transcribe audio segments via Rust `macos_asr::transcribe()` instead of sending the WAV file to the Python server.

#### Scenario: macOS worker uses Rust ASR

- GIVEN the system is running on macOS
- WHEN a voice segment is ready in `run_worker`
- THEN the worker MUST call `macos_asr::transcribe(&samples)` to get text
- AND MUST POST the resulting text to Python `/translate_text` for MT+TTS

#### Scenario: macOS ASR error does not crash worker

- GIVEN the Rust ASR returns an error for a segment
- WHEN `run_worker` receives the error
- THEN it MUST emit a `PhraseEvent` with `error` set
- AND MUST continue processing the next segment

### Requirement: `/translate_text` Endpoint

The Python FastAPI server MUST expose `POST /translate_text` accepting `{"text": "..."}` and returning translated audio + text.

#### Scenario: Text translated and synthesized

- GIVEN the server is running with MT and TTS models loaded
- WHEN a POST with `{"text": "hola"}` is sent to `/translate_text`
- THEN the server MUST translate "hola" to "hello"
- AND MUST synthesize a WAV file via Piper
- AND MUST return `{"output_wav": "...", "translated_text": "hello"}`

#### Scenario: Empty text returns 422

- GIVEN the server is running
- WHEN a POST with `{"text": ""}` is sent to `/translate_text`
- THEN the server MUST respond with HTTP 422

### Requirement: Conditional Server Spawn

On macOS, `engine_server.rs` MUST spawn the Python server with MT+TTS only, skipping NeMo import entirely.

#### Scenario: macOS server healthy without NeMo

- GIVEN the system is macOS
- WHEN `spawn_server()` is called
- THEN the Python server MUST start and pass `/health` checks
- AND MUST NOT import `nemo` or require `nemo_toolkit`

### Requirement: Parakeet Model Download

Setup on macOS MUST download and extract the Parakeet V3 ONNX model (`tar.gz`) to the app data directory.

#### Scenario: First-time macOS setup downloads model

- GIVEN the macOS user runs setup and no Parakeet model exists
- WHEN the setup process reaches the model download step
- THEN it MUST download the Parakeet ONNX archive from the configured URL
- AND MUST extract it to the app data directory
- AND MUST emit `setup-progress` events during download

#### Scenario: Already-downloaded model skipped

- GIVEN the Parakeet model already exists in the app data directory
- WHEN setup runs on macOS
- THEN the download step MUST be skipped silently

## MODIFIED Requirements

### Requirement: Live Translation Pipeline

On macOS, the live translation pipeline splits into two hops: Rust ASR produces text → Python MT+TTS consumes text.
(Previously: a single Python pipeline handled ASR, MT, and TTS from an audio WAV file.)

#### Scenario: End-to-end macOS translation completes

- GIVEN a macOS live session is active
- WHEN a voice segment is captured, transcribed, and the text sent to `/translate_text`
- THEN the UI receives both `source_text` and `translated_text` via the `phrase` event
- AND the translated audio plays on the output device

## REMOVED Requirements

### Requirement: NeMo ASR on macOS

(Reason: NeMo has no Apple Silicon wheel; replaced by Rust `transcribe-rs` on macOS.)
(Migration: Setup on macOS no longer installs `nemo_toolkit[asr]`. The Python server skips `_get_asr()` warmup.)

## RENAMED Requirements

None.
