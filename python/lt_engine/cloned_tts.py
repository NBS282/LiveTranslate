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
# RMS normalisation
# ---------------------------------------------------------------------------

def _normalize_rms(audio: np.ndarray, target_rms: float = 0.1) -> np.ndarray:
    """Normalise *audio* so its RMS equals *target_rms* (no-op if silent)."""
    rms = np.sqrt(np.mean(np.square(audio)))
    if rms < 1e-6:  # effectively silent — leave as-is
        return audio
    return audio * (target_rms / rms)


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
    audio = _normalize_rms(audio, target_rms=0.1)

    with wave.open(out_wav, "wb") as wf:
        wf.setnchannels(1)
        wf.setsampwidth(2)  # 16-bit
        wf.setframerate(_MODEL_SAMPLE_RATE)
        pcm = (audio * 32767).clip(-32768, 32767).astype(np.int16)
        wf.writeframes(pcm.tobytes())
