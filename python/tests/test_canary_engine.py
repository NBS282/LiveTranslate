"""Canary AST engine routing. The model itself is mocked (3.5 GB download)."""
from types import SimpleNamespace

import lt_engine.pipeline as pipeline


class FakeCanary:
    def __init__(self, text=" Hello world."):
        self.text = text
        self.calls = []

    def transcribe(self, paths, **kwargs):
        self.calls.append((paths, kwargs))
        return [SimpleNamespace(text=self.text)]


def test_speech_translate_calls_canary_ast(monkeypatch):
    fake = FakeCanary()
    monkeypatch.setattr(pipeline, "_get_canary", lambda: fake)

    out = pipeline.speech_translate("in.wav")

    assert out == "Hello world."
    paths, kwargs = fake.calls[0]
    assert paths == ["in.wav"]
    assert kwargs["task"] == "ast"
    assert kwargs["source_lang"] == "es"
    assert kwargs["target_lang"] == "en"
    assert kwargs["pnc"] == "yes"


def test_translate_audio_routes_to_canary(monkeypatch, tmp_path):
    monkeypatch.setattr(pipeline, "translation_engine", lambda: "canary")
    monkeypatch.setattr(pipeline, "speech_translate", lambda p: "Hello there.")
    used = {}
    monkeypatch.setattr(
        pipeline, "synthesize", lambda text, out_wav: used.update(text=text)
    )

    result = pipeline.translate_audio("in.wav", str(tmp_path))

    assert result["translated_text"] == "Hello there."
    assert result["source_text"] == ""
    assert used["text"] == "Hello there."


def test_translate_audio_canary_empty_raises_no_text(monkeypatch, tmp_path):
    import pytest

    monkeypatch.setattr(pipeline, "translation_engine", lambda: "canary")
    monkeypatch.setattr(pipeline, "speech_translate", lambda p: "   ")

    with pytest.raises(ValueError, match="transcription produced no text"):
        pipeline.translate_audio("in.wav", str(tmp_path))


def test_translate_audio_legacy_path_unchanged(monkeypatch, tmp_path):
    monkeypatch.setattr(pipeline, "translation_engine", lambda: "legacy")
    monkeypatch.setattr(pipeline, "transcribe", lambda p: "hola")
    monkeypatch.setattr(pipeline, "translate", lambda t: "hello")
    used = {}
    monkeypatch.setattr(
        pipeline, "synthesize", lambda text, out_wav: used.update(text=text)
    )

    result = pipeline.translate_audio("in.wav", str(tmp_path))

    assert result["source_text"] == "hola"
    assert result["translated_text"] == "hello"
