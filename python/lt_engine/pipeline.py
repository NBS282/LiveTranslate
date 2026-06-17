"""Modular offline translation pipeline: STT -> MT -> TTS (CPU)."""
from __future__ import annotations
import os
import wave

# NLLB uses FLORES-200 codes like "spa_Latn", "eng_Latn".
_NLLB_CODE = {"es": "spa_Latn", "en": "eng_Latn"}
_VALID_NLLB = {"spa_Latn", "eng_Latn", "por_Latn", "fra_Latn", "deu_Latn"}


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


def transcribe(audio_path: str) -> str:
    """Transcribe a WAV audio file to text using Parakeet (CPU)."""
    from nemo.collections.asr.models import ASRModel
    asr = ASRModel.from_pretrained("nvidia/parakeet-tdt-0.6b-v3", map_location="cpu")
    out = asr.transcribe([audio_path])
    item = out[0]
    return getattr(item, "text", item)


def translate(text: str, src: str, tgt: str) -> str:
    """Translate text from src to tgt language using NLLB-200 (CPU)."""
    from transformers import AutoTokenizer, AutoModelForSeq2SeqLM
    name = "facebook/nllb-200-distilled-600M"
    tok = AutoTokenizer.from_pretrained(name)
    model = AutoModelForSeq2SeqLM.from_pretrained(name)
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
    from piper import PiperVoice
    voice = PiperVoice.load(_piper_voice_path())
    with wave.open(out_wav, "wb") as wf:
        voice.synthesize_wav(text, wf)
