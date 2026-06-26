# python/lt_engine/cloned_tts.py
"""Chatterbox Turbo voice cloning wrapper (CPU, f32 dtype patch, ~700 MB RAM).

The model and the reference audio path are kept as module-level singletons
so the server pays the load cost only once. Call warmup_cloned() at startup
if a profile already exists, and reset_voice_prompt() after deletion.

Chatterbox Turbo (Resemble AI, 350M params, MIT license) is a distilled
1-step decoder TTS model supporting zero-shot voice cloning and
paralinguistic tags (``[laugh]``, ``[cough]``, ``[chuckle]``).  It loads
reference audio from disk at generation time — no separate prompt encoding.
"""
from __future__ import annotations

import logging
import os
import threading
import wave
import numpy as np

from . import voice_profile as vp

_log = logging.getLogger(__name__)

_model = None
_voice_prompt: str | None = None  # stores the reference WAV path
_model_lock = threading.Lock()
_prompt_lock = threading.Lock()

# Chatterbox Turbo outputs at 24 kHz.
_MODEL_SAMPLE_RATE = 24000


# ---------------------------------------------------------------------------
# f32 dtype patch for CPU inference
# ---------------------------------------------------------------------------

def _patch_torch_load():
    """Monkey-patch ``torch.load`` to default ``map_location="cpu"``.

    Chatterbox Turbo weights may have been saved with CUDA metadata.
    This ensures they load on CPU without raising a device mismatch error.
    Returns the original ``torch.load`` so callers can restore it.
    """
    import torch

    orig = torch.load

    def _patched(*args, **kwargs):
        kwargs.setdefault("map_location", "cpu")
        return orig(*args, **kwargs)

    torch.load = _patched
    return orig


def _restore_torch_load(orig):
    """Restore the original ``torch.load`` after model setup."""
    import torch

    torch.load = orig


# ---------------------------------------------------------------------------
# f32 dtype patch for librosa-loaded float64 audio (voicebox reference)
# ---------------------------------------------------------------------------

def _patch_chatterbox_f32(model) -> None:
    """Patch float64→float32 dtype mismatches in upstream chatterbox.

    libsora.load returns float64 numpy arrays. Multiple upstream code paths
    convert these to torch tensors via ``torch.from_numpy()`` without casting,
    then matmul against float32 model weights. This patches the two known
    entry points:

    1. ``S3Tokenizer.log_mel_spectrogram`` — audio tensor hits ``_mel_filters`` (f32)
    2. ``VoiceEncoder.forward`` — float64 mel spectrograms hit LSTM weights (f32)
    """
    import types

    # Patch S3Tokenizer
    _tokzr = model.s3gen.tokenizer
    _orig_log_mel = _tokzr.log_mel_spectrogram.__func__

    def _f32_log_mel(self_tokzr, audio, padding=0):
        import torch as _torch

        if _torch.is_tensor(audio):
            audio = audio.float()
        return _orig_log_mel(self_tokzr, audio, padding)

    _tokzr.log_mel_spectrogram = types.MethodType(_f32_log_mel, _tokzr)

    # Patch VoiceEncoder
    _ve = model.ve
    _orig_ve_forward = _ve.forward.__func__

    def _f32_ve_forward(self_ve, mels):
        return _orig_ve_forward(self_ve, mels.float())

    _ve.forward = types.MethodType(_f32_ve_forward, _ve)


# ---------------------------------------------------------------------------
# Model singleton
# ---------------------------------------------------------------------------

def _get_model():
    """Return the Chatterbox Turbo model singleton (lazy init, thread-safe)."""
    global _model
    if _model is None:
        with _model_lock:
            if _model is None:  # double-checked locking
                import torch
                from huggingface_hub import snapshot_download
                from chatterbox.tts_turbo import ChatterboxTurboTTS

                local_path = snapshot_download(repo_id="ResembleAI/chatterbox-turbo")
                _log.info("Chatterbox Turbo model downloaded at %s", local_path)

                orig_load = _patch_torch_load()
                try:
                    _model = ChatterboxTurboTTS.from_local(local_path, device="cpu")
                finally:
                    _restore_torch_load(orig_load)

                _patch_chatterbox_f32(_model)
                _log.info("Chatterbox Turbo model loaded on CPU (f32 patch applied)")
    return _model


# ---------------------------------------------------------------------------
# Voice prompt (reference audio path)
# ---------------------------------------------------------------------------

def get_voice_prompt() -> str | None:
    """Return the cached reference audio path, or *None* if no profile."""
    global _voice_prompt
    if _voice_prompt is None and vp.exists():
        with _prompt_lock:
            if _voice_prompt is None:
                _voice_prompt = str(vp.reference_path())
    return _voice_prompt


def reset_voice_prompt() -> None:
    """Clear the cached reference path (call after profile deletion)."""
    global _voice_prompt
    with _prompt_lock:
        _voice_prompt = None


# ---------------------------------------------------------------------------
# Warmup
# ---------------------------------------------------------------------------

def warmup_engine() -> None:
    """Load the Chatterbox Turbo model eagerly.

    Called unconditionally at server startup so the model weights
    (~700 MB download, ~seconds to init) are ready before any request.
    Does NOT resolve the voice prompt (that requires a profile).
    """
    _get_model()


def warmup_cloned() -> None:
    """Load model + resolve reference path. No-op if no profile exists."""
    if vp.exists():
        get_voice_prompt()


# ---------------------------------------------------------------------------
# Audio post-processing (ported from voicebox reference implementation)
# ---------------------------------------------------------------------------

def _normalize_audio(
    audio: np.ndarray,
    target_db: float = -20.0,
    peak_limit: float = 0.85,
) -> np.ndarray:
    """Normalize to target loudness (dB RMS) with peak limiting."""
    audio = audio.astype(np.float32)
    rms = np.sqrt(np.mean(audio ** 2))
    if rms > 0:
        target_rms = 10 ** (target_db / 20)
        audio = audio * (target_rms / rms)
    return np.clip(audio, -peak_limit, peak_limit)


def _trim_tts_output(
    audio: np.ndarray,
    sample_rate: int = 24000,
    frame_ms: int = 20,
    silence_threshold_db: float = -40.0,
    min_silence_ms: int = 200,
    max_internal_silence_ms: int = 1000,
    fade_ms: int = 30,
) -> np.ndarray:
    """Trim trailing silence and hallucinated noise from Chatterbox output.

    Chatterbox Turbo sometimes produces [speech][silence][hallucinated_noise].
    This detects internal silence gaps > max_internal_silence_ms and cuts there,
    then trims trailing silence and applies a short cosine fade-out.
    """
    frame_len = int(sample_rate * frame_ms / 1000)
    if frame_len == 0 or len(audio) < frame_len:
        return audio

    n_frames = len(audio) // frame_len
    threshold_linear = 10 ** (silence_threshold_db / 20)

    rms = np.array(
        [
            np.sqrt(np.mean(audio[i * frame_len : (i + 1) * frame_len] ** 2))
            for i in range(n_frames)
        ]
    )
    is_speech = rms >= threshold_linear

    first_speech = 0
    for i, s in enumerate(is_speech):
        if s:
            first_speech = max(0, i - 1)
            break

    max_silence_frames = int(max_internal_silence_ms / frame_ms)
    consecutive_silence = 0
    cut_frame = n_frames

    for i in range(first_speech, n_frames):
        if is_speech[i]:
            consecutive_silence = 0
        else:
            consecutive_silence += 1
            if consecutive_silence >= max_silence_frames:
                cut_frame = i - consecutive_silence + 1
                break

    min_silence_frames = int(min_silence_ms / frame_ms)
    end_frame = cut_frame
    while end_frame > first_speech and not is_speech[end_frame - 1]:
        end_frame -= 1
    end_frame = min(end_frame + min_silence_frames, cut_frame)

    start_sample = first_speech * frame_len
    end_sample = min(end_frame * frame_len, len(audio))
    trimmed = audio[start_sample:end_sample].copy()

    fade_samples = int(sample_rate * fade_ms / 1000)
    if fade_samples > 0 and len(trimmed) > fade_samples:
        fade = np.cos(np.linspace(0, np.pi / 2, fade_samples)) ** 2
        trimmed[-fade_samples:] *= fade

    return trimmed


def _preprocess_reference_audio(
    audio: np.ndarray,
    sample_rate: int,
    peak_target: float = 0.95,
    trim_top_db: float = 40.0,
    edge_padding_ms: int = 100,
) -> np.ndarray:
    """Clean up reference audio before passing to voice cloning.

    Removes DC offset, trims leading/trailing silence, and caps peak so a
    slightly-hot recording doesn't distort the cloned voice.
    """
    try:
        import librosa
    except ImportError:
        return audio.astype(np.float32)

    audio = audio.astype(np.float32, copy=False)
    if audio.size == 0:
        return audio

    audio = audio - float(np.mean(audio))

    trimmed, _ = librosa.effects.trim(audio, top_db=trim_top_db)
    if 0 < trimmed.size < audio.size:
        pad_each = int(sample_rate * edge_padding_ms / 1000)
        headroom = (audio.size - trimmed.size) // 2
        pad = min(pad_each, max(headroom, 0))
        if pad > 0:
            trimmed = np.pad(trimmed, (pad, pad), mode="constant")
        audio = trimmed

    peak = float(np.abs(audio).max())
    if peak > peak_target and peak > 0:
        audio = audio * (peak_target / peak)

    return audio


# ---------------------------------------------------------------------------
# Fallback
# ---------------------------------------------------------------------------

def _fallback(text: str, out_wav: str) -> None:
    """Synthesize via Piper (non-cloned) — shared fallback path."""
    from .pipeline import synthesize

    synthesize(text, out_wav)


# ---------------------------------------------------------------------------
# Main synthesis
# ---------------------------------------------------------------------------

def synthesize_cloned(text: str, out_wav: str) -> None:
    """Synthesize *text* in the cloned voice and write a 16-bit mono WAV.

    Falls back silently to Piper when:
    * No voice profile is available (no reference audio).
    * The model raises an exception during generation.

    Chatterbox Turbo loads the reference audio from disk at generation time.
    The output sample rate is 24 kHz (down from LuxTTS's 48 kHz).
    """
    ref = get_voice_prompt()
    if ref is None:
        _fallback(text, out_wav)
        return

    model = _get_model()
    if model is None:
        _fallback(text, out_wav)
        return

    try:
        wav = model.generate(
            text,
            audio_prompt_path=ref,
            temperature=0.8,
            top_k=1000,
            top_p=0.95,
            repetition_penalty=1.2,
        )
    except Exception as exc:
        _log.warning("Chatterbox Turbo generation failed: %s", exc)
        _fallback(text, out_wav)
        return

    audio: np.ndarray = wav.squeeze().cpu().numpy().astype(np.float32)
    audio = _trim_tts_output(audio, sample_rate=_MODEL_SAMPLE_RATE)
    audio = _normalize_audio(audio, target_db=-20.0, peak_limit=0.85)

    with wave.open(out_wav, "wb") as wf:
        wf.setnchannels(1)
        wf.setsampwidth(2)  # 16-bit
        wf.setframerate(_MODEL_SAMPLE_RATE)
        pcm = (audio * 32767).clip(-32768, 32767).astype(np.int16)
        wf.writeframes(pcm.tobytes())
