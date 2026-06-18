"""Persistent FastAPI translation server. Loads models once at startup. Binds 127.0.0.1 only."""
import os
from contextlib import asynccontextmanager

from fastapi import FastAPI, HTTPException
from pydantic import BaseModel

from .pipeline import translate_audio, warmup


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


@app.get("/health")
def health() -> dict:
    return {"ready": True}


@app.post("/translate")
def do_translate(req: TranslateRequest) -> dict:
    if not os.path.isfile(req.input_path):
        raise HTTPException(status_code=400, detail=f"input not found: {req.input_path}")
    try:
        return translate_audio(req.input_path, req.out_dir, req.src, req.tgt)
    except ValueError as e:
        raise HTTPException(status_code=422, detail=str(e))
    except Exception as e:
        import traceback
        traceback.print_exc()  # full traceback to server stderr for diagnosis
        raise HTTPException(status_code=500, detail=f"translation failed: {e}")


def main() -> None:
    import uvicorn
    port = int(os.environ.get("LT_ENGINE_PORT", "8765"))
    uvicorn.run(app, host="127.0.0.1", port=port, log_level="info")


if __name__ == "__main__":
    main()
