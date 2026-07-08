"""Persistent FastAPI translation server. Loads models once at startup. Binds 127.0.0.1 only."""
import os
import threading
from contextlib import asynccontextmanager

from fastapi import FastAPI, HTTPException, Request
from pydantic import BaseModel

from .pipeline import (
    cloning_available,
    cloning_error,
    speech_translate,
    translate_audio,
    translation_engine,
    warmup,
)
from . import voice_profile as vp
from .cloned_tts import reset_voice_state, export_voice_state, preprocess_reference_audio


@asynccontextmanager
async def lifespan(app: FastAPI):
    warmup()
    yield


app = FastAPI(lifespan=lifespan)


class TranslateRequest(BaseModel):
    input_path: str
    out_dir: str
    src: str = "es"
    tgt: str = "en"
    use_cloned_voice: bool = False


class PartialRequest(BaseModel):
    input_path: str


@app.get("/health")
def health() -> dict:
    return {"ready": True, "cloning_available": cloning_available()}


@app.post("/translate")
def do_translate(req: TranslateRequest) -> dict:
    if not os.path.isfile(req.input_path):
        raise HTTPException(status_code=400, detail=f"input not found: {req.input_path}")
    try:
        return translate_audio(
            req.input_path,
            req.out_dir,
            req.src,
            req.tgt,
            req.use_cloned_voice,
        )
    except ValueError as e:
        raise HTTPException(status_code=422, detail=str(e))
    except Exception as e:
        import traceback
        traceback.print_exc()
        raise HTTPException(status_code=500, detail=f"translation failed: {e}")


@app.post("/transcribe-partial")
def transcribe_partial(req: PartialRequest) -> dict:
    """Translate an in-progress (open) audio segment. Display-only partials."""
    if not os.path.isfile(req.input_path):
        raise HTTPException(status_code=400, detail=f"input not found: {req.input_path}")
    if translation_engine() != "canary":
        # Legacy engine has no cheap partial-decode path. Lazy-loading Canary
        # here would defeat the rollback guarantee (3.5GB model load under
        # LT_TRANSLATION_ENGINE=legacy). Empty text is the established
        # "nothing to show" signal the Rust caller swallows.
        return {"text": ""}
    try:
        return {"text": speech_translate(req.input_path, allow_bisect=False)}
    except Exception as e:
        import traceback
        traceback.print_exc()
        raise HTTPException(status_code=500, detail=f"partial decode failed: {e}")


@app.get("/voice-profile")
def get_voice_profile() -> dict:
    return {"exists": vp.exists()}


@app.post("/voice-profile")
async def upload_voice_profile(request: Request) -> dict:
    """Save reference audio for voice cloning.

    Accepts raw WAV bytes in the request body (Content-Type: audio/wav).
    After saving, preprocesses the audio and pre-computes the Pocket TTS
    voice state as .safetensors so the first generation is fast.
    """
    if not cloning_available():
        reason = cloning_error() or "voice cloning engine failed to load"
        raise HTTPException(
            status_code=503,
            detail=f"voice cloning unavailable: {reason}",
        )

    audio_bytes = await request.body()
    if not audio_bytes:
        raise HTTPException(status_code=400, detail="empty audio file")

    vp.save(audio_bytes)

    # Preprocess: remove DC offset, trim silence edges, normalize peak.
    try:
        import io
        import soundfile as sf

        buf = io.BytesIO(audio_bytes)
        audio, sr = sf.read(buf, dtype="float32", always_2d=False)
        if audio.ndim == 2:
            audio = audio.mean(axis=1)
        cleaned = preprocess_reference_audio(audio, sr)
        sf.write(str(vp.reference_path()), cleaned, sr)
    except Exception:
        pass  # keep original if preprocessing fails

    reset_voice_state()  # clear cache so next synthesis reloads from disk

    # Pre-compute .safetensors in the background; don't block the HTTP response.
    # A plain daemon thread (not asyncio.create_task) is used on purpose: the
    # event loop only keeps a weak reference to tasks, so a fire-and-forget task
    # can be garbage-collected mid-run. If this export fails (OOM, missing
    # weights) the client still gets a success response and the next synthesis
    # falls back to computing the voice state from the raw WAV.
    threading.Thread(target=export_voice_state, daemon=True).start()

    return {"exists": True}


@app.delete("/voice-profile")
def delete_voice_profile() -> dict:
    vp.delete()
    reset_voice_state()
    return {"exists": False}


if __name__ == "__main__":
    import uvicorn

    port = int(os.environ.get("LT_ENGINE_PORT", "8765"))
    uvicorn.run(
        "lt_engine.server:app",
        host="127.0.0.1",
        port=port,
        log_level="info",
    )
