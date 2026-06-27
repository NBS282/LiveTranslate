# python/lt_engine/cloned_tts.py
"""Pocket TTS voice cloning wrapper (CPU, ~200ms first chunk, 100M params).

The model and the voice state are module-level singletons so the server
pays the load cost only once. Call warmup_engine() at startup.
Call export_voice_state() after saving a new profile so the next
generation loads from .safetensors (fast) instead of reprocessing the WAV.
"""
from __future__ import annotations

import logging
import threading
import wave
import numpy as np

from . import voice_profile as vp

_log = logging.getLogger(__name__)

_model = None
_voice_state = None
_model_lock = threading.Lock()
_state_lock = threading.Lock()

# ---------------------------------------------------------------------------
# Model singleton
# ---------------------------------------------------------------------------

def _get_model():
    """Return the Pocket TTS model singleton (lazy init, thread-safe)."""
    global _model
    if _model is None:
        with _model_lock:
            if _model is None:
                from pocket_tts import TTSModel
                model = TTSModel.load_model()
                # Pad short inputs with leading spaces. Pocket TTS degrades the
                # first words when the token count is very low (<5 words), which
                # is the common case for subtitle-length phrases. Upstream gates
                # this behind a config flag that defaults to False; enable it so
                # the opening words are intelligible.
                model.pad_with_spaces_for_short_inputs = True
                _model = model
                _log.info("Pocket TTS model loaded (short-input padding ON)")
    return _model


# ---------------------------------------------------------------------------
# Voice state management
# ---------------------------------------------------------------------------

def _state_path():
    """Pre-computed voice state file (.safetensors) alongside the WAV."""
    return vp.profile_dir() / "reference.safetensors"


def get_voice_state():
    """Return the cached voice state, computing it on first access.

    Prefers loading from .safetensors (fast) over reprocessing the WAV.
    Returns None if no voice profile exists.
    """
    global _voice_state
    if _voice_state is None and vp.exists():
        with _state_lock:
            if _voice_state is None:
                model = _get_model()
                st = _state_path()
                src = str(st) if st.exists() else str(vp.reference_path())
                _voice_state = model.get_state_for_audio_prompt(src)
                _log.info("Voice state loaded from %s", src)
    return _voice_state


def reset_voice_state() -> None:
    """Clear the cached voice state (call after profile save or deletion)."""
    global _voice_state
    with _state_lock:
        _voice_state = None


def export_voice_state() -> None:
    """Pre-compute voice state from the reference WAV and save as .safetensors.

    Call this after uploading new reference audio so subsequent calls to
    get_voice_state() use the fast .safetensors path instead of reprocessing
    the raw WAV (which is relatively slow per the Pocket TTS docs).
    """
    if not vp.exists():
        return
    try:
        from pocket_tts import export_model_state
        model = _get_model()
        state = model.get_state_for_audio_prompt(str(vp.reference_path()))
        export_model_state(state, str(_state_path()))
        _log.info("Voice state exported to %s", _state_path())
    except Exception as exc:
        _log.warning("Voice state export failed (non-fatal): %s", exc)


# ---------------------------------------------------------------------------
# Warmup
# ---------------------------------------------------------------------------

def warmup_engine() -> None:
    """Load the Pocket TTS model eagerly at server startup."""
    _get_model()


def warmup_cloned() -> None:
    """Load model + resolve voice state. No-op if no profile exists."""
    if vp.exists():
        get_voice_state()


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
    """Trim trailing silence and hallucinated noise from TTS output.

    Detects internal silence gaps > max_internal_silence_ms and cuts there,
    then trims trailing silence and applies a cosine fade-out.
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

    # Keep ~60 ms before the first detected speech frame so soft onset
    # consonants (s, f, th) that dip below the threshold are not clipped.
    lead_pad_frames = max(1, int(60 / frame_ms))
    first_speech = 0
    for i, s in enumerate(is_speech):
        if s:
            first_speech = max(0, i - lead_pad_frames)
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

    # Prepend a short silence so audio players / sinks that ramp up on the
    # first samples don't swallow the opening phoneme.
    lead_silence = np.zeros(int(sample_rate * 0.03), dtype=trimmed.dtype)
    return np.concatenate([lead_silence, trimmed])


def preprocess_reference_audio(
    audio: np.ndarray,
    sample_rate: int,
    peak_target: float = 0.95,
    trim_top_db: float = 40.0,
    edge_padding_ms: int = 100,
) -> np.ndarray:
    """Clean up reference audio before voice cloning.

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
    * No voice profile is available.
    * The model raises an exception during generation.

    Pocket TTS uses a pre-computed voice state (loaded from .safetensors
    when available) to minimize per-call overhead.
    """
    voice_state = get_voice_state()
    if voice_state is None:
        _fallback(text, out_wav)
        return

    model = _get_model()
    try:
        wav = model.generate_audio(voice_state, text)
    except Exception as exc:
        _log.warning("Pocket TTS generation failed: %s", exc)
        _fallback(text, out_wav)
        return

    # generate_audio returns a 1-D torch tensor
    if hasattr(wav, "numpy"):
        audio = wav.squeeze().numpy().astype(np.float32)
    else:
        audio = np.asarray(wav, dtype=np.float32).squeeze()

    sr: int = getattr(model, "sample_rate", 24000)
    audio = _trim_tts_output(audio, sample_rate=sr)
    audio = _normalize_audio(audio, target_db=-20.0, peak_limit=0.85)

    with wave.open(out_wav, "wb") as wf:
        wf.setnchannels(1)
        wf.setsampwidth(2)  # 16-bit
        wf.setframerate(sr)
        pcm = (audio * 32767).clip(-32768, 32767).astype(np.int16)
        wf.writeframes(pcm.tobytes())
