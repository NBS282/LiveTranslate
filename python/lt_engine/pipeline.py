"""Modular offline translation pipeline: STT -> MT -> TTS (CPU).

Models are loaded lazily on first use and kept in module-level singletons
so the server process pays the load cost only once.

Translation uses Helsinki-NLP/opus-mt-es-en, a MarianMT model trained
specifically for ES->EN. It outperforms the general-purpose NLLB-200-distilled
on this language pair and is faster at inference (dedicated model, smaller vocab).
"""
from __future__ import annotations
import os
import wave

_asr = None
_mt = None
_piper = None


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
    """Load all models eagerly. Call once at server startup."""
    _get_asr()
    _get_mt()
    _get_piper()


def translate_audio(
    input_path: str,
    out_dir: str,
    src: str = "es",
    tgt: str = "en",
) -> dict:
    """Run the full STT -> MT -> TTS pipeline on a WAV file.

    Args:
        input_path: Path to the source WAV file.
        out_dir: Directory where output.wav will be written.
        src: Unused — model is ES->EN only, kept for API compatibility.
        tgt: Unused — model is ES->EN only, kept for API compatibility.

    Returns:
        dict with keys: output_wav, source_text, translated_text.

    Raises:
        ValueError: if transcription produces no text.
    """
    os.makedirs(out_dir, exist_ok=True)

    source_text = transcribe(input_path)
    if not source_text.strip():
        raise ValueError("transcription produced no text")

    translated_text = translate(source_text)
    out_wav = os.path.join(out_dir, "output.wav")
    synthesize(translated_text, out_wav)

    return {
        "output_wav": out_wav,
        "source_text": source_text,
        "translated_text": translated_text,
    }
