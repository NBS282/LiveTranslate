# macos-asr Specification

## Purpose

On-device Spanish speech-to-text on macOS Apple Silicon via `transcribe-rs` wrapping a Parakeet V3 ONNX model with Metal GPU acceleration. Replaces the Python NeMo ASR pipeline on macOS.

## Requirements

### Requirement: Parakeet Model Loading

The system MUST load the Parakeet V3 int8 ONNX model via `transcribe-rs::ParakeetModel::load()` from a configurable app data directory.

#### Scenario: Model loads successfully

- GIVEN the Parakeet ONNX model file exists in the app data directory
- WHEN `ParakeetModel::load()` is called
- THEN the model handle MUST be returned and ready for inference

#### Scenario: Model file missing at load time

- GIVEN the Parakeet ONNX model file is NOT present
- WHEN `ParakeetModel::load()` is called
- THEN the module MUST return `Err` with a descriptive message including the expected path

### Requirement: Audio Transcription

The system MUST transcribe 16 kHz mono PCM audio (`Vec<i16>`) to Spanish text using the loaded Parakeet model.

#### Scenario: Clear Spanish speech transcribed

- GIVEN a `Vec<i16>` buffer holding 2+ seconds of clear Spanish speech at 16 kHz
- WHEN `transcribe()` is called with the buffer
- THEN it MUST return the corresponding Spanish text string

#### Scenario: Silence or noise returns empty

- GIVEN a `Vec<i16>` buffer containing only silence or non-speech noise
- WHEN `transcribe()` is called
- THEN it MAY return an empty string or a transcription error

### Requirement: Metal GPU Inference

On Apple Silicon, the `whisper-metal` feature MUST be enabled for Metal-accelerated inference.

#### Scenario: Inference runs on Metal

- GIVEN the system is Apple Silicon (`aarch64-apple-darwin`)
- WHEN the model runs inference
- THEN transcribe-rs MUST execute via Metal GPU (verified via crate logging or performance characteristics)

### Requirement: Error Handling

The module MUST wrap all `transcribe-rs` errors into `Result<String, String>` and surface them to the caller.

#### Scenario: Inference error propagated

- GIVEN the Parakeet model encounters an error (OOM, shape mismatch, etc.)
- WHEN the error surfaces from transcribe-rs
- THEN the module MUST return `Err` with the original error context

### Requirement: Thread Safety

The module MUST be `Send` so it can be called from the worker thread in `live.rs`.

#### Scenario: Cross-thread usage compiles

- GIVEN the module is used inside a `std::thread::spawn` closure
- WHEN the Rust compiler checks thread safety
- THEN compilation MUST succeed without `Send` or `Sync` violations
