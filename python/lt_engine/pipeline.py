"""Modular offline translation pipeline: STT -> MT -> TTS (CPU).

Models are loaded lazily on first use and kept in module-level singletons
so the server process pays the load cost only once.
"""
from __future__ import annotations
import os
import wave

# NLLB uses FLORES-200 codes like "spa_Latn", "eng_Latn".
_NLLB_CODE = {"es": "spa_Latn", "en": "eng_Latn"}
_VALID_NLLB = {"spa_Latn", "eng_Latn", "por_Latn", "fra_Latn", "deu_Latn"}

_asr = None
_nllb = None
_piper = None


def normalize_lang(code: str) -> str:
    """Map a short code ('es') to its NLLB code, or validate a full NLLB code."""
    if "_" in code:
        if code not in _VALID_NLLB:
            raise ValueError(f"unknown NLLB code: {code}")
        return code
    if code in _NLLB_CODE:
        return _NLLB_CODE[code]
    raise ValueError(f"unknown language code: {code}")


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


def _get_nllb():
    global _nllb
    if _nllb is None:
        from transformers import AutoTokenizer, AutoModelForSeq2SeqLM
        name = "facebook/nllb-200-distilled-600M"
        _nllb = (AutoTokenizer.from_pretrained(name), AutoModelForSeq2SeqLM.from_pretrained(name))
    return _nllb


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


def translate(text: str, src: str, tgt: str) -> str:
    """Translate text from src to tgt language using NLLB-200 (CPU)."""
    tok, model = _get_nllb()
    tok.src_lang = src
    inputs = tok(text, return_tensors="pt")
    gen = model.generate(
        **inputs,
        forced_bos_token_id=tok.convert_tokens_to_ids(tgt),
        max_length=512,
    )
    return tok.batch_decode(gen, skip_special_tokens=True)[0]


def synthesize(text: str, out_wav: str) -> None:
    """Synthesize English text to a WAV file using Piper TTS."""
    voice = _get_piper()
    with wave.open(out_wav, "wb") as wf:
        voice.synthesize_wav(text, wf)


def warmup() -> None:
    """Load all models eagerly. Call once at server startup."""
    _get_asr()
    _get_nllb()
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
        src: Source language short code or NLLB code.
        tgt: Target language short code or NLLB code.

    Returns:
        dict with keys: output_wav, source_text, translated_text.

    Raises:
        ValueError: if transcription produces no text.
    """
    os.makedirs(out_dir, exist_ok=True)
    s = normalize_lang(src)
    t = normalize_lang(tgt)

    source_text = transcribe(input_path)
    if not source_text.strip():
        raise ValueError("transcription produced no text")

    translated_text = translate(source_text, s, t)
    out_wav = os.path.join(out_dir, "output.wav")
    synthesize(translated_text, out_wav)

    return {
        "output_wav": out_wav,
        "source_text": source_text,
        "translated_text": translated_text,
    }
