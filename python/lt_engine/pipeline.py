"""Modular offline translation pipeline: STT -> MT -> TTS (CPU).

Models are loaded lazily on first use and kept in module-level singletons
so the server process pays the load cost only once.

Engine: Parakeet TDT 0.6B v3 (multilingual ASR, via NeMo) -> MarianMT (one
Helsinki-NLP/opus-mt model per language direction). MarianMT outperforms the
general-purpose NLLB-200-distilled per pair and is faster at inference
(dedicated models, smaller vocab).

Canary 1B Flash direct speech translation (AST) used to be a selectable
second engine here (LT_TRANSLATION_ENGINE=canary). It was replaced by a
native `transcribe_cpp` GGUF engine on the Rust side (see
`src-tauri/src/translation/engine/native_canary.rs`) and removed from this
module; this pipeline now always runs the cascade path.

STT backend: `transcribe.cpp` (native, GGUF Parakeet) is the default STT
engine as of `engine::build` in `src-tauri/src/translation/engine/mod.rs` —
this process only needs to load NeMo's Parakeet ASR when `LT_STT_BACKEND`
resolves to `"python"` (the explicit opt-out). See `_stt_backend_is_native`.
"""
from __future__ import annotations
import os
import threading
import wave

_asr = None
_mt_models: dict[tuple[str, str], tuple] = {}
_mt_lock = threading.Lock()
_piper = None

# NeMo's transcribe() mutates shared model/decoder state. /translate and
# /transcribe-partial run on separate FastAPI threadpool threads and can call
# it concurrently, causing a data race. Serialize all decodes through this lock.
_decode_lock = threading.Lock()

# Voice cloning availability, resolved once during warmup(). Cloning is an
# optional feature: if Pocket TTS cannot load (package missing, gated model
# not cached and download rejected), the server keeps running with Piper.
_cloning_available = False
_cloning_error: str | None = None

# Warmup progress, polled via /health while models load. Counts completed
# warmup tasks — with parallel loading there is no single "current step".
_warmup_progress_lock = threading.Lock()
_warmup_tasks_done = 0
_WARMUP_TASKS_TOTAL = 3


def warmup_progress() -> int:
    """Percentage of warmup tasks completed (0-100)."""
    with _warmup_progress_lock:
        return int(100 * _warmup_tasks_done / _WARMUP_TASKS_TOTAL)


def _mark_warmup_task_done() -> None:
    global _warmup_tasks_done
    with _warmup_progress_lock:
        _warmup_tasks_done += 1


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


def _stt_backend_is_native() -> bool:
    """Mirrors the Rust side's `LT_STT_BACKEND` resolution (native default,
    "python" is the only explicit opt-out — see `backend_choice` in
    `src-tauri/src/translation/engine/mod.rs`) so this process agrees with
    the Rust engine selection on whether NeMo Parakeet ASR needs to load.

    `engine_server::spawn_server` sets this env var explicitly on this
    process before it starts, so "unset" only happens when the server is run
    standalone (e.g. under pytest) — defaulting to native there too keeps
    both sides' defaults identical.
    """
    return os.environ.get("LT_STT_BACKEND", "native") != "python"


def _get_asr():
    global _asr
    if _asr is None:
        import os
        # NeMo respects NEMO_CACHE_DIR; mirror HF_HOME so models stay in the app dir.
        hf_home = os.environ.get("HF_HOME")
        if hf_home and "NEMO_CACHE_DIR" not in os.environ:
            os.environ["NEMO_CACHE_DIR"] = os.path.join(hf_home, "nemo")
        try:
            from nemo.collections.asr.models import ASRModel
        except ImportError as e:
            raise RuntimeError(
                "python STT fallback requires nemo_toolkit; native STT "
                "(transcribe.cpp) is the default. Install nemo_toolkit[asr] "
                "manually to use LT_STT_BACKEND=python."
            ) from e
        _asr = ASRModel.from_pretrained("nvidia/parakeet-tdt-0.6b-v3", map_location="cpu")
    return _asr


def ensure_asr_loaded() -> None:
    """Load the NeMo ASR model now (blocking, possibly minutes cold).

    Called by the Rust side right after it falls back from native STT to this
    sidecar: under the native-default warmup the ASR was deliberately never
    warmed, and letting the first /translate request absorb the cold load
    would blow its 120s client timeout. Serialized under _decode_lock so a
    concurrent decode cannot double-load.

    Raises:
        RuntimeError: when nemo_toolkit is not installed (fresh installs).
    """
    with _decode_lock:
        _get_asr()


# One MarianMT model per translation direction, matching the same six pairs
# the native Canary AST engine supports so both accept the same UI selector.
_MARIAN_MODELS = {
    ("es", "en"): "Helsinki-NLP/opus-mt-es-en",
    ("en", "es"): "Helsinki-NLP/opus-mt-en-es",
    ("fr", "en"): "Helsinki-NLP/opus-mt-fr-en",
    ("en", "fr"): "Helsinki-NLP/opus-mt-en-fr",
    ("de", "en"): "Helsinki-NLP/opus-mt-de-en",
    ("en", "de"): "Helsinki-NLP/opus-mt-en-de",
}


def _get_mt(src: str = "es", tgt: str = "en"):
    """MarianMT (tokenizer, model) for one direction, cached per pair."""
    pair = (src, tgt)
    if pair not in _mt_models:
        with _mt_lock:
            if pair not in _mt_models:
                from transformers import MarianMTModel, MarianTokenizer
                name = _MARIAN_MODELS[pair]
                _mt_models[pair] = (
                    MarianTokenizer.from_pretrained(name),
                    MarianMTModel.from_pretrained(name),
                )
    return _mt_models[pair]


def _get_piper():
    global _piper
    if _piper is None:
        from piper import PiperVoice
        _piper = PiperVoice.load(_piper_voice_path())
    return _piper


# Supported translation directions (EN<->DE/ES/FR): one MarianMT model exists
# per pair (see `_MARIAN_MODELS`); anything else is rejected at the boundary.
SUPPORTED_LANGUAGE_PAIRS = frozenset(
    {("es", "en"), ("en", "es"), ("de", "en"), ("en", "de"), ("fr", "en"), ("en", "fr")}
)


def validate_language_pair(src: str, tgt: str) -> tuple[str, str]:
    """Normalize and validate a translation pair against the pairs MarianMT
    is loaded for.

    Returns:
        The normalized (source, target) tuple.

    Raises:
        ValueError: if the pair is not a supported direction.
    """
    pair = (src.strip().lower(), tgt.strip().lower())
    if pair not in SUPPORTED_LANGUAGE_PAIRS:
        supported = ", ".join(sorted(f"{s}->{t}" for s, t in SUPPORTED_LANGUAGE_PAIRS))
        raise ValueError(
            f"unsupported language pair: {src}->{tgt} (supported: {supported})"
        )
    return pair


def transcribe(audio_path: str) -> str:
    """Transcribe a WAV audio file to text using Parakeet (CPU).

    Serialized through the decode lock: Parakeet is NeMo too, and with
    cascade partials enabled, /translate and /transcribe-partial can call
    this concurrently from separate FastAPI threadpool threads.
    """
    with _decode_lock:
        out = _get_asr().transcribe([audio_path])
    item = out[0]
    return getattr(item, "text", item)


def translate(text: str, src: str = "es", tgt: str = "en") -> str:
    """Translate text with the MarianMT model for the given direction (CPU).

    Raises:
        ValueError: if the language pair is not supported.
    """
    src, tgt = validate_language_pair(src, tgt)
    tok, model = _get_mt(src, tgt)
    inputs = tok([text], return_tensors="pt", padding=True, truncation=True, max_length=512)
    gen = model.generate(**inputs, max_length=512)
    return tok.batch_decode(gen, skip_special_tokens=True)[0]


def transcribe_translate(
    audio_path: str, source_lang: str = "es", target_lang: str = "en"
) -> str:
    """Cascade decode of one WAV: Parakeet transcript -> Marian translation.

    Empty string means no speech — the established "nothing to show" signal.

    Raises:
        ValueError: if the language pair is not supported.
    """
    source_lang, target_lang = validate_language_pair(source_lang, target_lang)
    text = transcribe(audio_path)
    if not text.strip():
        return ""
    return translate(text, source_lang, target_lang)


def synthesize(text: str, out_wav: str) -> None:
    """Synthesize English text to a WAV file using Piper TTS."""
    voice = _get_piper()
    with wave.open(out_wav, "wb") as wf:
        voice.synthesize_wav(text, wf)


def synthesize_reply(text: str, out_dir: str, use_cloned_voice: bool = False) -> str:
    """Synthesize the reply audio for already-translated `text`.

    This is the exact piper/pocket-tts routing `translate_audio` runs after
    translation: cloned voice when requested and available, falling back to
    Piper on any cloning failure so a voice hiccup only costs quality, never
    the reply itself. Extracted so `/tts` (native-STT composition) and
    `translate_audio` (Python-sidecar composition) share one code path.

    Args:
        text: Already-translated text to speak.
        out_dir: Directory the output WAV is written into (created if missing).
        use_cloned_voice: If True and a voice profile exists, use Chatterbox
            Turbo instead of Piper for synthesis.

    Returns:
        Path to the written `output.wav`.
    """
    os.makedirs(out_dir, exist_ok=True)
    out_wav = os.path.join(out_dir, "output.wav")

    if use_cloned_voice and _cloning_available:
        from .cloned_tts import synthesize_cloned
        try:
            synthesize_cloned(text, out_wav)
        except Exception:
            # A cloned-voice hiccup (e.g. voice state still being exported)
            # must cost one phrase's voice quality, never the translation.
            import traceback
            traceback.print_exc()
            synthesize(text, out_wav)
    else:
        synthesize(text, out_wav)

    return out_wav


def _load_core_models() -> None:
    """Load the cascade engine's models (fatal on failure).

    Always warms the default es->en Marian; other directions load lazily on
    the first request that selects them. Parakeet ASR is only warmed here
    when `LT_STT_BACKEND` resolves to "python" — with native `transcribe.cpp`
    as the default STT backend, this process doesn't pay NeMo's ASR load
    (the whole point of the native cascade) unless it is the primary STT
    path or the explicit fallback target. If a native session falls back to
    this sidecar later, `_get_asr` lazy-loads on the first `/translate`
    request instead — slower for that one segment, but functional.
    """
    if not _stt_backend_is_native():
        _get_asr()
    _get_mt()


def _load_cloning() -> None:
    """Load Pocket TTS best-effort: any failure is recorded and the server
    starts without cloning instead of dying. A dead server surfaces to the UI
    as a bare connection error on every request, which is far worse than a
    degraded feature."""
    global _cloning_available, _cloning_error
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
        return
    # Pre-resolve an existing profile's voice state so the first cloned
    # synthesis after a restart doesn't pay it inside the request. Purely an
    # optimization: on failure the state still resolves lazily on demand.
    try:
        from .cloned_tts import warmup_cloned
        warmup_cloned()
    except Exception:  # noqa: BLE001 — lazy path remains as fallback
        import traceback
        traceback.print_exc()


def warmup() -> None:
    """Load the cascade engine's models eagerly. Call once at server startup.

    The three loads (core engine, Piper, Pocket TTS) are independent and run
    in parallel threads: weight loading releases the GIL in native code, so
    the smaller models hide under Parakeet's load instead of adding to it.
    Set LT_WARMUP_PARALLEL=0 to force the sequential path (low-RAM machines).

    A core-model failure still propagates out of warmup() — the Rust side
    relies on the process dying fast instead of hanging until timeout.
    """
    global _warmup_tasks_done
    tasks = (_load_core_models, _get_piper, _load_cloning)
    with _warmup_progress_lock:
        _warmup_tasks_done = 0

    def run(task) -> None:
        task()
        _mark_warmup_task_done()

    if os.environ.get("LT_WARMUP_PARALLEL", "1") == "0":
        for task in tasks:
            run(task)
        return

    from concurrent.futures import ThreadPoolExecutor

    with ThreadPoolExecutor(max_workers=len(tasks)) as pool:
        futures = [pool.submit(run, task) for task in tasks]
        for future in futures:
            future.result()  # re-raises core/Piper failures


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
        src: Spoken language.
        tgt: Language to translate into.
        use_cloned_voice: If True and a voice profile exists, use Chatterbox
            Turbo instead of Piper for synthesis.

    Returns:
        dict with keys: output_wav, source_text, translated_text.

    Raises:
        ValueError: if transcription produces no text, or the language pair
            is unsupported.
    """
    os.makedirs(out_dir, exist_ok=True)

    source_text = transcribe(input_path)
    if not source_text.strip():
        raise ValueError("transcription produced no text")
    translated_text = translate(source_text, src, tgt)

    out_wav = synthesize_reply(translated_text, out_dir, use_cloned_voice)

    return {
        "output_wav": out_wav,
        "source_text": source_text,
        "translated_text": translated_text,
    }
