"""Warmup resilience: a Pocket TTS failure must never kill the server.

Regression tests for the v0.2.0 production incident: the shipped HF token
expired, the gated Pocket TTS model could not be downloaded, warmup() raised,
and the FastAPI server never bound its port — every request then failed with
"error sending request" (connection refused).
"""
import pytest

import lt_engine.cloned_tts as cloned_tts
import lt_engine.pipeline as pipeline


def _patch_core_models(monkeypatch):
    monkeypatch.setattr(pipeline, "_get_asr", lambda: object())
    monkeypatch.setattr(pipeline, "_get_mt", lambda: object())
    monkeypatch.setattr(pipeline, "_get_piper", lambda: object())
    monkeypatch.setattr(pipeline, "_get_canary", lambda: object())
    # Keep tests hermetic: never resolve a real voice profile from disk.
    monkeypatch.setattr(cloned_tts, "warmup_cloned", lambda: None)


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


def test_warmup_propagates_core_model_failure(monkeypatch):
    """A core model failure (Canary/Piper) must still abort warmup: the Rust
    side relies on the process dying fast instead of hanging until timeout."""
    _patch_core_models(monkeypatch)
    monkeypatch.setattr(cloned_tts, "warmup_engine", lambda: None)

    def boom():
        raise RuntimeError("piper voice file is corrupt")

    monkeypatch.setattr(pipeline, "_get_piper", boom)

    with pytest.raises(RuntimeError, match="corrupt"):
        pipeline.warmup()


def test_warmup_resolves_existing_voice_state(monkeypatch):
    """warmup() must pre-resolve an existing voice profile's state so the
    first cloned synthesis after a restart doesn't pay it in-request."""
    _patch_core_models(monkeypatch)
    monkeypatch.setattr(cloned_tts, "warmup_engine", lambda: None)
    called = {}
    monkeypatch.setattr(
        cloned_tts, "warmup_cloned", lambda: called.setdefault("cloned", True)
    )

    pipeline.warmup()

    assert called.get("cloned") is True
    assert pipeline.cloning_available() is True


def test_warmup_cloned_failure_keeps_cloning_available(monkeypatch):
    """Voice-state pre-resolution is an optimization: if it fails, cloning
    stays available and the state resolves lazily on the first request."""
    _patch_core_models(monkeypatch)
    monkeypatch.setattr(cloned_tts, "warmup_engine", lambda: None)

    def boom():
        raise RuntimeError("voice state export still in progress")

    monkeypatch.setattr(cloned_tts, "warmup_cloned", boom)

    pipeline.warmup()  # must not raise

    assert pipeline.cloning_available() is True
    assert pipeline.cloning_error() is None


def test_warmup_sequential_fallback_loads_everything(monkeypatch):
    """LT_WARMUP_PARALLEL=0 forces the sequential path (support escape hatch
    for low-RAM machines); every model must still load."""
    monkeypatch.setenv("LT_WARMUP_PARALLEL", "0")
    calls = []
    monkeypatch.setattr(pipeline, "translation_engine", lambda: "canary")
    monkeypatch.setattr(pipeline, "_get_canary", lambda: calls.append("canary"))
    monkeypatch.setattr(pipeline, "_get_asr", lambda: calls.append("asr"))
    monkeypatch.setattr(pipeline, "_get_mt", lambda: calls.append("mt"))
    monkeypatch.setattr(pipeline, "_get_piper", lambda: calls.append("piper"))
    monkeypatch.setattr(cloned_tts, "warmup_engine", lambda: calls.append("pocket"))
    monkeypatch.setattr(cloned_tts, "warmup_cloned", lambda: None)

    pipeline.warmup()

    assert "canary" in calls and "piper" in calls and "pocket" in calls
    assert "asr" not in calls and "mt" not in calls


def test_translate_audio_falls_back_to_piper_when_cloning_unavailable(
    monkeypatch, tmp_path
):
    monkeypatch.setattr(pipeline, "translation_engine", lambda: "legacy")
    monkeypatch.setattr(pipeline, "transcribe", lambda p: "hola mundo")
    monkeypatch.setattr(pipeline, "translate", lambda t, *a, **k: "hello world")
    used = {}

    def fake_piper(text, out_wav):
        used["engine"] = "piper"

    monkeypatch.setattr(pipeline, "synthesize", fake_piper)
    monkeypatch.setattr(pipeline, "_cloning_available", False)

    pipeline.translate_audio("in.wav", str(tmp_path), use_cloned_voice=True)

    assert used["engine"] == "piper"


def test_synthesize_reply_uses_piper_by_default(monkeypatch, tmp_path):
    used = {}
    monkeypatch.setattr(
        pipeline, "synthesize", lambda text, out_wav: used.update(text=text, out_wav=out_wav)
    )

    out_dir = tmp_path / "reply"
    result = pipeline.synthesize_reply("hello world", str(out_dir))

    assert result == str(out_dir / "output.wav")
    assert used == {"text": "hello world", "out_wav": str(out_dir / "output.wav")}


def test_synthesize_reply_uses_cloned_voice_when_available(monkeypatch, tmp_path):
    monkeypatch.setattr(pipeline, "_cloning_available", True)
    used = {}
    monkeypatch.setattr(
        cloned_tts,
        "synthesize_cloned",
        lambda text, out_wav: used.update(text=text, out_wav=out_wav),
    )

    out_dir = tmp_path / "reply"
    result = pipeline.synthesize_reply("hello world", str(out_dir), use_cloned_voice=True)

    assert result == str(out_dir / "output.wav")
    assert used == {"text": "hello world", "out_wav": str(out_dir / "output.wav")}


def test_synthesize_reply_falls_back_to_piper_on_cloning_failure(monkeypatch, tmp_path):
    monkeypatch.setattr(pipeline, "_cloning_available", True)

    def boom(text, out_wav):
        raise RuntimeError("voice state load failed")

    monkeypatch.setattr(cloned_tts, "synthesize_cloned", boom)
    used = {}
    monkeypatch.setattr(
        pipeline, "synthesize", lambda text, out_wav: used.update(engine="piper")
    )

    result = pipeline.synthesize_reply("hello world", str(tmp_path), use_cloned_voice=True)

    assert used["engine"] == "piper"
    assert result == str(tmp_path / "output.wav")


def test_synthesize_reply_creates_out_dir(monkeypatch, tmp_path):
    monkeypatch.setattr(pipeline, "synthesize", lambda text, out_wav: None)
    out_dir = tmp_path / "nested" / "reply"

    pipeline.synthesize_reply("hello world", str(out_dir))

    assert out_dir.is_dir()
