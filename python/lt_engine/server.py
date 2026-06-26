"""Persistent FastAPI translation server. Loads models once at startup. Binds 127.0.0.1 only."""
import asyncio
import os
from contextlib import asynccontextmanager

from fastapi import FastAPI, HTTPException, UploadFile, File
from pydantic import BaseModel

from .pipeline import translate_audio, warmup
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


@app.get("/health")
def health() -> dict:
    return {"ready": True}


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


@app.get("/voice-profile")
def get_voice_profile() -> dict:
    return {"exists": vp.exists()}


@app.post("/voice-profile")
async def upload_voice_profile(file: UploadFile = File(...)) -> dict:
    """Save reference audio for voice cloning.

    After saving the WAV, preprocesses the audio (DC offset, silence trim,
    peak cap) and pre-computes the Pocket TTS voice state as .safetensors
    so the first generation is fast.
    """
    audio_bytes = await file.read()
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

    # Pre-compute voice state → .safetensors (non-blocking).
    await asyncio.to_thread(export_voice_state)

    reset_voice_state()  # next get_voice_state() loads the fresh .safetensors
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
