"""Persistent FastAPI translation server. Loads models once at startup. Binds 127.0.0.1 only."""
import asyncio
import os
from contextlib import asynccontextmanager

from fastapi import FastAPI, HTTPException, UploadFile, File
from pydantic import BaseModel

from .pipeline import translate_audio, warmup
from . import voice_profile as vp
from .cloned_tts import reset_voice_prompt


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

    Chatterbox Turbo loads the reference audio from disk at generation
    time — no separate prompt encoding step needed.
    """
    audio_bytes = await file.read()
    if not audio_bytes:
        raise HTTPException(status_code=400, detail="empty audio file")
    vp.save(audio_bytes)
    reset_voice_prompt()  # clear any stale cached path
    return {"exists": True}


@app.delete("/voice-profile")
def delete_voice_profile() -> dict:
    vp.delete()
    reset_voice_prompt()
    return {"exists": False}


def main() -> None:
    import uvicorn
    port = int(os.environ.get("LT_ENGINE_PORT", "8765"))
    uvicorn.run(app, host="127.0.0.1", port=port, log_level="info")


if __name__ == "__main__":
    main()
