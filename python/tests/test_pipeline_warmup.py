"""Warmup resilience: a Pocket TTS failure must never kill the server.

Regression tests for the v0.2.0 production incident: the shipped HF token
expired, the gated Pocket TTS model could not be downloaded, warmup() raised,
and the FastAPI server never bound its port — every request then failed with
"error sending request" (connection refused).
"""
import lt_engine.cloned_tts as cloned_tts
import lt_engine.pipeline as pipeline


def _patch_core_models(monkeypatch):
    monkeypatch.setattr(pipeline, "_get_asr", lambda: object())
    monkeypatch.setattr(pipeline, "_get_mt", lambda: object())
    monkeypatch.setattr(pipeline, "_get_piper", lambda: object())


def test_warmup_survives_cloned_tts_failure(monkeypatch):
    _patch_core_models(monkeypatch)

    def boom():
        raise RuntimeError("401 Client Error: gated repo requires a valid token")

    monkeypatch.setattr(cloned_tts, "warmup_engine", boom)

    pipeline.warmup()  # must not raise

    assert pipeline.cloning_available() is False
    assert "401" in pipeline.cloning_error()


def test_warmup_marks_cloning_available_on_success(monkeypatch):
    _patch_core_models(monkeypatch)
    monkeypatch.setattr(cloned_tts, "warmup_engine", lambda: None)

    pipeline.warmup()

    assert pipeline.cloning_available() is True
    assert pipeline.cloning_error() is None


def test_translate_audio_falls_back_to_piper_when_cloning_unavailable(
    monkeypatch, tmp_path
):
    monkeypatch.setattr(pipeline, "translation_engine", lambda: "legacy")
    monkeypatch.setattr(pipeline, "transcribe", lambda p: "hola mundo")
    monkeypatch.setattr(pipeline, "translate", lambda t: "hello world")
    used = {}

    def fake_piper(text, out_wav):
        used["engine"] = "piper"

    monkeypatch.setattr(pipeline, "synthesize", fake_piper)
    monkeypatch.setattr(pipeline, "_cloning_available", False)

    pipeline.translate_audio("in.wav", str(tmp_path), use_cloned_voice=True)

    assert used["engine"] == "piper"
