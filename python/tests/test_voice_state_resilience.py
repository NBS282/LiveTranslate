"""Voice state race resilience.

Regression tests for the "500 after cloning until app restart" incident:
export_voice_state() runs in a background thread after profile upload and
wrote the .safetensors directly to its final path. A translation requested
during that window loaded the half-written file and failed — every phrase
returned 500 until a restart (by which time the export had finished).
"""
import sys
import types
from pathlib import Path

import lt_engine.cloned_tts as ct
import lt_engine.pipeline as pipeline


def _fake_profile(monkeypatch, tmp_path):
    wav = tmp_path / "reference.wav"
    wav.write_bytes(b"RIFF-fake")
    monkeypatch.setattr(ct.vp, "exists", lambda: True)
    monkeypatch.setattr(ct.vp, "profile_dir", lambda: tmp_path)
    monkeypatch.setattr(ct.vp, "reference_path", lambda: wav)
    return wav


def test_export_voice_state_writes_atomically(monkeypatch, tmp_path):
    _fake_profile(monkeypatch, tmp_path)

    class FakeModel:
        def get_state_for_audio_prompt(self, src):
            return {"src": src}

    monkeypatch.setattr(ct, "_get_model", lambda: FakeModel())

    seen = {}

    def fake_export(state, path):
        seen["path"] = path
        Path(path).write_bytes(b"tensor-data")

    monkeypatch.setitem(
        sys.modules, "pocket_tts", types.SimpleNamespace(export_model_state=fake_export)
    )

    ct.export_voice_state()

    final = tmp_path / "reference.safetensors"
    assert final.exists()
    assert final.read_bytes() == b"tensor-data"
    # The writer must never touch the final path directly — a concurrent
    # reader would see a partial file. It writes elsewhere, then renames.
    assert seen["path"] != str(final)
    assert not Path(seen["path"]).exists()


def test_get_voice_state_rebuilds_from_wav_when_safetensors_corrupt(
    monkeypatch, tmp_path
):
    wav = _fake_profile(monkeypatch, tmp_path)
    corrupt = tmp_path / "reference.safetensors"
    corrupt.write_bytes(b"half-written garbage")

    class FakeModel:
        def get_state_for_audio_prompt(self, src):
            if src.endswith(".safetensors"):
                raise RuntimeError("HeaderTooSmall")
            return f"state-from:{src}"

    monkeypatch.setattr(ct, "_get_model", lambda: FakeModel())
    ct.reset_voice_state()
    try:
        state = ct.get_voice_state()

        assert state == f"state-from:{wav}"
        assert not corrupt.exists(), "corrupt export should be discarded"
    finally:
        ct.reset_voice_state()


def test_translate_audio_uses_piper_when_cloned_synthesis_raises(
    monkeypatch, tmp_path
):
    monkeypatch.setattr(pipeline, "translation_engine", lambda: "legacy")
    monkeypatch.setattr(pipeline, "transcribe", lambda p: "hola mundo")
    monkeypatch.setattr(pipeline, "translate", lambda t, *a, **k: "hello world")
    monkeypatch.setattr(pipeline, "_cloning_available", True)

    def boom(text, out_wav):
        raise RuntimeError("voice state load failed")

    monkeypatch.setattr(ct, "synthesize_cloned", boom)

    used = {}
    monkeypatch.setattr(
        pipeline, "synthesize", lambda text, out_wav: used.update(engine="piper")
    )

    result = pipeline.translate_audio("in.wav", str(tmp_path), use_cloned_voice=True)

    assert used["engine"] == "piper"
    assert result["translated_text"] == "hello world"
