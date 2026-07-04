# python/lt_engine/cloned_tts.py
"""Pocket TTS voice cloning wrapper (CPU, ~200ms first chunk, 100M params).

The model and the voice state are module-level singletons so the server
pays the load cost only once. Call warmup_engine() at startup.
Call export_voice_state() after saving a new profile so the next
generation loads from .safetensors (fast) instead of reprocessing the WAV.
"""
from __future__ import annotations

import logging
import os
import re
import threading
import wave
import numpy as np

from . import voice_profile as vp

_log = logging.getLogger(__name__)

_model = None
_voice_state = None
_model_lock = threading.Lock()
_state_lock = threading.Lock()


def _import_pocket_tts():
    """Import pocket_tts without letting it throttle the process.

    pocket_tts calls torch.set_num_threads(1) at import time, which would
    degrade every other model in this process (Canary/Parakeet decode gets
    ~3x slower). Save and restore the thread count around the import.
    """
    import torch

    n_threads = torch.get_num_threads()
    import pocket_tts

    torch.set_num_threads(n_threads)
    return pocket_tts


# ---------------------------------------------------------------------------
# Model singleton
# ---------------------------------------------------------------------------

def _get_model():
    """Return the Pocket TTS model singleton (lazy init, thread-safe)."""
    global _model
    if _model is None:
        with _model_lock:
            if _model is None:
                TTSModel = _import_pocket_tts().TTSModel
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
                if st.exists():
                    try:
                        _voice_state = model.get_state_for_audio_prompt(str(st))
                        _log.info("Voice state loaded from %s", st)
                    except Exception as exc:
                        # Half-written or corrupt export — discard it and
                        # rebuild from the WAV instead of failing translation.
                        _log.warning(
                            "Voice state file unreadable (%s); rebuilding from WAV", exc
                        )
                        try:
                            st.unlink()
                        except OSError:
                            pass
                if _voice_state is None:
                    src = str(vp.reference_path())
                    _voice_state = model.get_state_for_audio_prompt(src)
                    _log.info("Voice state computed from %s", src)
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
        export_model_state = _import_pocket_tts().export_model_state
        model = _get_model()
        state = model.get_state_for_audio_prompt(str(vp.reference_path()))
        # Write to a temp file, then rename atomically: this runs in a
        # background thread and a concurrent get_voice_state() must never
        # see a half-written .safetensors.
        final = _state_path()
        tmp = final.with_name(final.name + ".tmp")
        export_model_state(state, str(tmp))
        os.replace(tmp, final)
        _log.info("Voice state exported to %s", final)
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
    fade_ms: int = 30,
) -> np.ndarray:
    """Trim leading and trailing silence from TTS output, then fade out.

    NOTE: this deliberately does NOT cut at internal silence gaps. Pocket TTS
    splits long text into per-sentence chunks and concatenates them, so the
    pauses between sentences are legitimate content — cutting at the first long
    gap would drop every sentence after it.
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

    if not is_speech.any():
        return audio  # all silence — nothing to trim

    # Keep ~60 ms before the first detected speech frame so soft onset
    # consonants (s, f, th) that dip below the threshold are not clipped.
    lead_pad_frames = max(1, int(60 / frame_ms))
    first_speech = max(0, int(np.argmax(is_speech)) - lead_pad_frames)

    # Last frame that contains speech, plus a short trailing silence pad.
    last_speech = n_frames - 1 - int(np.argmax(is_speech[::-1]))
    min_silence_frames = int(min_silence_ms / frame_ms)
    end_frame = min(last_speech + 1 + min_silence_frames, n_frames)

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
# Sentence splitting
# ---------------------------------------------------------------------------

_SENTENCE_RE = re.compile(r"[^.!?]+(?:[.!?]+|$)")


def _split_sentences(text: str) -> list[str]:
    """Split *text* into sentences on . ! ? boundaries.

    Falls back to the whole text as a single sentence when no boundary is
    found. Empty fragments are dropped.
    """
    text = text.strip()
    if not text:
        return []
    parts = [m.group().strip() for m in _SENTENCE_RE.finditer(text)]
    parts = [p for p in parts if p]
    return parts or [text]


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
    sr: int = getattr(model, "sample_rate", 24000)

    # Generate one sentence at a time. Pocket TTS groups several sentences into a
    # single chunk when they fit under its token budget, and in that case the
    # autoregressive decoder emits EOS early and truncates the tail (the last
    # sentence is dropped). Splitting ourselves guarantees each sentence is
    # generated to completion; we then concatenate with a short pause.
    sentences = _split_sentences(text)
    pause = np.zeros(int(sr * 0.15), dtype=np.float32)  # 150 ms between sentences

    pieces: list[np.ndarray] = []
    try:
        for sentence in sentences:
            wav = model.generate_audio(voice_state, sentence)
            if hasattr(wav, "numpy"):
                piece = wav.squeeze().numpy().astype(np.float32)
            else:
                piece = np.asarray(wav, dtype=np.float32).squeeze()
            piece = _trim_tts_output(piece, sample_rate=sr)
            pieces.append(piece)
    except Exception as exc:
        _log.warning("Pocket TTS generation failed: %s", exc)
        _fallback(text, out_wav)
        return

    if not pieces:
        _fallback(text, out_wav)
        return

    # Join sentences with a short pause between them.
    joined: list[np.ndarray] = []
    for i, piece in enumerate(pieces):
        if i > 0:
            joined.append(pause)
        joined.append(piece)
    audio = np.concatenate(joined)
    audio = _normalize_audio(audio, target_db=-20.0, peak_limit=0.85)

    with wave.open(out_wav, "wb") as wf:
        wf.setnchannels(1)
        wf.setsampwidth(2)  # 16-bit
        wf.setframerate(sr)
        pcm = (audio * 32767).clip(-32768, 32767).astype(np.int16)
        wf.writeframes(pcm.tobytes())
