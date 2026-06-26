from __future__ import annotations

import os
import shutil
from pathlib import Path


def profile_dir() -> Path:
    root = os.environ.get("LT_ENGINE_ROOT")
    if root:
        base = Path(root)
    else:
        # Dev mode: use repo root (two levels up from this file)
        base = Path(__file__).parent.parent.parent
    return base / "voice_profile"


def reference_path() -> Path:
    return profile_dir() / "reference.wav"


def exists() -> bool:
    return reference_path().is_file()


def save(audio_bytes: bytes) -> None:
    d = profile_dir()
    d.mkdir(parents=True, exist_ok=True)
    reference_path().write_bytes(audio_bytes)


def delete() -> None:
    d = profile_dir()
    if d.exists():
        shutil.rmtree(d)
