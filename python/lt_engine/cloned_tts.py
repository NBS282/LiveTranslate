# python/lt_engine/cloned_tts.py
"""LuxTTS voice cloning wrapper (CPU ONNX int8, ~180 MB RAM, ~15 ms/sentence).

The model and the encoded voice prompt are kept as module-level singletons
so the server pays the load cost only once. Call warmup_cloned() at startup
if a profile already exists, and reset_voice_prompt() after deletion.
"""
from __future__ import annotations

import os
import threading
import wave
import numpy as np

from . import voice_profile as vp

_luxtts = None
_voice_prompt: dict | None = None
_luxtts_lock = threading.Lock()


def _get_luxtts():
    global _luxtts
    if _luxtts is None:
        with _luxtts_lock:
            if _luxtts is None:  # double-checked locking
                from zipvoice.luxvoice import LuxTTS

                hf_repo = "YatharthS/LuxTTS"
                threads = min(os.cpu_count() or 4, 8)
                _luxtts = LuxTTS(model_path=hf_repo, device="cpu", threads=threads)
    return _luxtts


def _encode_reference() -> dict:
    """Encode the stored reference WAV into a LuxTTS voice prompt dict."""
    model = _get_luxtts()
    ref = str(vp.reference_path())
    return model.encode_prompt(prompt_audio=ref, duration=5, rms=0.01)


def get_voice_prompt() -> dict | None:
    """Return cached voice prompt, encoding on first call if profile exists."""
    global _voice_prompt
    if _voice_prompt is None and vp.exists():
        _voice_prompt = _encode_reference()
    return _voice_prompt


def reset_voice_prompt() -> None:
    """Clear the cached voice prompt (call after profile deletion)."""
    global _voice_prompt
    _voice_prompt = None


def warmup_cloned() -> None:
    """Load model + encode reference. No-op if no profile exists."""
    if vp.exists():
        get_voice_prompt()


def synthesize_cloned(text: str, out_wav: str) -> None:
    """Synthesize *text* in the cloned voice and write a WAV to *out_wav*.

    Falls back silently to Piper if no voice prompt is available.
    """
    prompt = get_voice_prompt()
    if prompt is None:
        from .pipeline import synthesize
        synthesize(text, out_wav)
        return

    model = _get_luxtts()
    wav_tensor = model.generate_speech(
        text=text,
        encode_dict=prompt,
        num_steps=4,
        guidance_scale=3.0,
        t_shift=0.5,
        speed=1.0,
        return_smooth=False,
    )
    audio: np.ndarray = wav_tensor.detach().cpu().numpy().squeeze()
    sample_rate = 48_000

    with wave.open(out_wav, "wb") as wf:
        wf.setnchannels(1)
        wf.setsampwidth(2)  # 16-bit
        wf.setframerate(sample_rate)
        pcm = (audio * 32767).clip(-32768, 32767).astype(np.int16)
        wf.writeframes(pcm.tobytes())
