"""Modular offline translation pipeline: STT -> MT -> TTS (CPU).

Models are loaded lazily on first use and kept in module-level singletons
so the server process pays the load cost only once.

Translation uses Helsinki-NLP/opus-mt-es-en, a MarianMT model trained
specifically for ES->EN. It outperforms the general-purpose NLLB-200-distilled
on this language pair and is faster at inference (dedicated model, smaller vocab).
"""
from __future__ import annotations
import os
import re
import threading
import wave

_asr = None
_mt = None
_piper = None
_canary = None

# NeMo's transcribe() mutates shared model/decoder state. /translate and
# /transcribe-partial run on separate FastAPI threadpool threads and can call
# it concurrently, causing a data race. Serialize all Canary decodes through
# this lock.
_decode_lock = threading.Lock()

# Voice cloning availability, resolved once during warmup(). Cloning is an
# optional feature: if Pocket TTS cannot load (package missing, gated model
# not cached and download rejected), the server keeps running with Piper.
_cloning_available = False
_cloning_error: str | None = None


def cloning_available() -> bool:
    """True if the Pocket TTS engine loaded successfully during warmup."""
    return _cloning_available


def cloning_error() -> str | None:
    """Human-readable reason cloning is unavailable, or None."""
    return _cloning_error


def _piper_voice_path() -> str:
    """Resolve the Piper ONNX voice: env override, else repo-root default."""
    env = os.environ.get("PIPER_VOICE")
    if env:
        return env
    here = os.path.dirname(os.path.abspath(__file__))   # python/lt_engine
    repo_root = os.path.dirname(os.path.dirname(here))  # repo root
    return os.path.join(repo_root, "en_US-lessac-medium.onnx")


def _get_asr():
    global _asr
    if _asr is None:
        import os
        # NeMo respects NEMO_CACHE_DIR; mirror HF_HOME so models stay in the app dir.
        hf_home = os.environ.get("HF_HOME")
        if hf_home and "NEMO_CACHE_DIR" not in os.environ:
            os.environ["NEMO_CACHE_DIR"] = os.path.join(hf_home, "nemo")
        from nemo.collections.asr.models import ASRModel
        _asr = ASRModel.from_pretrained("nvidia/parakeet-tdt-0.6b-v3", map_location="cpu")
    return _asr


def _get_mt():
    global _mt
    if _mt is None:
        from transformers import MarianMTModel, MarianTokenizer
        name = "Helsinki-NLP/opus-mt-es-en"
        _mt = (MarianTokenizer.from_pretrained(name), MarianMTModel.from_pretrained(name))
    return _mt


def _get_piper():
    global _piper
    if _piper is None:
        from piper import PiperVoice
        _piper = PiperVoice.load(_piper_voice_path())
    return _piper


def translation_engine() -> str:
    """Active live-translation engine: "canary" (default) or "legacy"."""
    return os.environ.get("LT_TRANSLATION_ENGINE", "canary")


def _get_canary():
    global _canary
    if _canary is None:
        from nemo.collections.asr.models import EncDecMultiTaskModel

        model = EncDecMultiTaskModel.from_pretrained("nvidia/canary-1b-flash")
        model.eval()
        cfg = model.cfg.decoding
        # Greedy by default: beam search collapses to an empty decode on some
        # real-speech inputs (immediate EOS), and greedy is also ~35% faster.
        # The quality delta measured on real samples was marginal.
        cfg.beam.beam_size = int(os.environ.get("LT_CANARY_BEAM", "1"))
        model.change_decoding_strategy(cfg)
        _canary = model
    return _canary


_SPECIAL_TOKEN = re.compile(r"<\|[^|>]*\|>")

# Below this duration an empty decode is almost certainly a breath tail or
# silence — retrying in halves would only waste CPU on the live path.
_BISECT_MIN_SECONDS = 4.0


def _decode_ast(audio_path: str) -> str:
    """Raw Canary AST decode of one WAV, sanitized. Empty string = no speech."""
    with _decode_lock:
        out = _get_canary().transcribe(
            [audio_path],
            source_lang="es",
            target_lang="en",
            task="ast",
            pnc="yes",
            batch_size=1,
            verbose=False,
        )
    item = out[0]
    text = _SPECIAL_TOKEN.sub("", getattr(item, "text", item)).strip()
    # Near-silent clips make the decoder emit degenerate output (raw special
    # tokens, stray punctuation). If nothing alphanumeric survives, treat the
    # segment as silence so the caller skips it instead of speaking garbage.
    if not any(c.isalnum() for c in text):
        return ""
    return text


def _decode_or_bisect(audio, sample_rate: int, depth: int) -> str:
    """Decode a clip; on empty output, split in halves and recover the parts.

    Canary AST can emit nothing for a whole segment when a short span inside
    it derails the decoder (observed with English terms embedded in Spanish
    speech, e.g. "plan-driven"). Bisecting confines the loss to the vicinity
    of the poison span instead of dropping many seconds of real speech.
    """
    import tempfile

    import soundfile as sf

    with tempfile.NamedTemporaryFile(suffix=".wav", delete=False) as tmp:
        path = tmp.name
    try:
        sf.write(path, audio, sample_rate)
        text = _decode_ast(path)
    finally:
        try:
            os.unlink(path)
        except OSError:
            pass

    duration = len(audio) / sample_rate
    if text or depth <= 0 or duration < _BISECT_MIN_SECONDS:
        return text

    mid = len(audio) // 2
    left = _decode_or_bisect(audio[:mid], sample_rate, depth - 1)
    right = _decode_or_bisect(audio[mid:], sample_rate, depth - 1)
    return " ".join(part for part in (left, right) if part)


def speech_translate(audio_path: str, allow_bisect: bool = True) -> str:
    """Translate Spanish speech directly to English text (Canary AST).

    The clip is peak-normalized before decoding: quiet mic captures
    (peak ~0.1) make Canary collapse to an empty or degenerate decode that
    the model handles fine at normal levels.

    Args:
        audio_path: Path to a 16 kHz WAV file.
        allow_bisect: Recover empty decodes of long clips by decoding halves.
            Disable on the partial-decode hot path, where an empty result is
            transient and two extra decodes per tick would starve finals.
    """
    import numpy as np
    import soundfile as sf

    audio, sample_rate = sf.read(audio_path, dtype="float32", always_2d=False)
    if audio.ndim == 2:
        audio = audio.mean(axis=1)
    peak = float(np.max(np.abs(audio))) if len(audio) else 0.0
    if peak > 1e-4:
        audio = audio * (0.9 / peak)

    return _decode_or_bisect(audio, sample_rate, depth=1 if allow_bisect else 0)


def transcribe(audio_path: str) -> str:
    """Transcribe a WAV audio file to text using Parakeet (CPU)."""
    out = _get_asr().transcribe([audio_path])
    item = out[0]
    return getattr(item, "text", item)


def translate(text: str) -> str:
    """Translate Spanish text to English using opus-mt-tc-big-es-en (CPU)."""
    tok, model = _get_mt()
    inputs = tok([text], return_tensors="pt", padding=True, truncation=True, max_length=512)
    gen = model.generate(**inputs, max_length=512)
    return tok.batch_decode(gen, skip_special_tokens=True)[0]


def synthesize(text: str, out_wav: str) -> None:
    """Synthesize English text to a WAV file using Piper TTS."""
    voice = _get_piper()
    with wave.open(out_wav, "wb") as wf:
        voice.synthesize_wav(text, wf)


def warmup() -> None:
    """Load the active engine's models eagerly. Call once at server startup.

    Pocket TTS (voice cloning) is loaded best-effort: any failure is recorded
    and the server starts without cloning instead of dying. A dead server
    surfaces to the UI as a bare connection error on every request, which is
    far worse than a degraded feature.
    """
    global _cloning_available, _cloning_error
    if translation_engine() == "canary":
        _get_canary()
    else:
        _get_asr()
        _get_mt()
    _get_piper()
    try:
        from .cloned_tts import warmup_engine
        warmup_engine()
        _cloning_available = True
        _cloning_error = None
    except Exception as e:  # noqa: BLE001 — degradation boundary, reason kept
        import traceback
        traceback.print_exc()
        _cloning_available = False
        _cloning_error = f"{type(e).__name__}: {e}"


def translate_audio(
    input_path: str,
    out_dir: str,
    src: str = "es",
    tgt: str = "en",
    use_cloned_voice: bool = False,
) -> dict:
    """Run the full STT -> MT -> TTS pipeline on a WAV file.

    Args:
        input_path: Path to the source WAV file.
        out_dir: Directory where output.wav will be written.
        src: Unused — model is ES->EN only, kept for API compatibility.
        tgt: Unused — model is ES->EN only, kept for API compatibility.
        use_cloned_voice: If True and a voice profile exists, use Chatterbox
            Turbo instead of Piper for synthesis.

    Returns:
        dict with keys: output_wav, source_text, translated_text.

    Raises:
        ValueError: if transcription produces no text.
    """
    os.makedirs(out_dir, exist_ok=True)

    if translation_engine() == "canary":
        # Canary AST: speech -> translated text in one pass. There is no
        # intermediate Spanish transcript to show.
        source_text = ""
        translated_text = speech_translate(input_path)
        if not translated_text.strip():
            raise ValueError("transcription produced no text")
    else:
        source_text = transcribe(input_path)
        if not source_text.strip():
            raise ValueError("transcription produced no text")
        translated_text = translate(source_text)
    out_wav = os.path.join(out_dir, "output.wav")

    # Fall back to Piper when cloning was requested but the engine is not
    # available — translation must keep working even if cloning is degraded.
    if use_cloned_voice and _cloning_available:
        from .cloned_tts import synthesize_cloned
        try:
            synthesize_cloned(translated_text, out_wav)
        except Exception:
            # A cloned-voice hiccup (e.g. voice state still being exported)
            # must cost one phrase's voice quality, never the translation.
            import traceback
            traceback.print_exc()
            synthesize(translated_text, out_wav)
    else:
        synthesize(translated_text, out_wav)

    return {
        "output_wav": out_wav,
        "source_text": source_text,
        "translated_text": translated_text,
    }
