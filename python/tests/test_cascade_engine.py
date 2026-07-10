"""Cascade engine (Parakeet ASR -> MarianMT text translation) routing.

The cascade is now the DEFAULT engine; Canary AST remains available via
LT_TRANSLATION_ENGINE=canary for quality comparison. Models are mocked.
"""
import threading
import time
from types import SimpleNamespace

import pytest

import lt_engine.pipeline as pipeline


def test_default_engine_is_cascade(monkeypatch):
    monkeypatch.delenv("LT_TRANSLATION_ENGINE", raising=False)
    assert pipeline.translation_engine() == "cascade"


def test_marian_map_covers_all_supported_pairs():
    assert set(pipeline._MARIAN_MODELS) == set(pipeline.SUPPORTED_LANGUAGE_PAIRS)


def test_translate_selects_model_for_pair(monkeypatch):
    seen = {}

    class FakeTok:
        def __call__(self, texts, **kw):
            return {}

        def batch_decode(self, gen, **kw):
            return ["hello world"]

    class FakeModel:
        def generate(self, **kw):
            return object()

    def fake_get_mt(src, tgt):
        seen["pair"] = (src, tgt)
        return FakeTok(), FakeModel()

    monkeypatch.setattr(pipeline, "_get_mt", fake_get_mt)

    out = pipeline.translate("hola mundo", "es", "en")

    assert out == "hello world"
    assert seen["pair"] == ("es", "en")


def test_translate_rejects_unsupported_pair():
    with pytest.raises(ValueError, match="unsupported language pair"):
        pipeline.translate("hola", "es", "de")


def test_transcribe_translate_chains_transcript_into_translation(monkeypatch):
    monkeypatch.setattr(pipeline, "transcribe", lambda p: "hola mundo")
    seen = {}

    def fake_translate(text, src="es", tgt="en"):
        seen["args"] = (text, src, tgt)
        return "hello world"

    monkeypatch.setattr(pipeline, "translate", fake_translate)

    out = pipeline.transcribe_translate("in.wav", "es", "en")

    assert out == "hello world"
    assert seen["args"] == ("hola mundo", "es", "en")


def test_transcribe_translate_empty_transcript_short_circuits(monkeypatch):
    monkeypatch.setattr(pipeline, "transcribe", lambda p: "   ")

    def must_not_translate(*args, **kwargs):
        raise AssertionError("translate must not run on an empty transcript")

    monkeypatch.setattr(pipeline, "translate", must_not_translate)

    assert pipeline.transcribe_translate("in.wav", "es", "en") == ""


def test_transcribe_translate_rejects_unsupported_pair():
    with pytest.raises(ValueError, match="unsupported language pair"):
        pipeline.transcribe_translate("in.wav", "es", "de")


def test_translate_audio_cascade_forwards_pair(monkeypatch, tmp_path):
    monkeypatch.setattr(pipeline, "translation_engine", lambda: "cascade")
    monkeypatch.setattr(pipeline, "transcribe", lambda p: "hola")
    seen = {}

    def fake_translate(text, src="es", tgt="en"):
        seen["args"] = (text, src, tgt)
        return "hello"

    monkeypatch.setattr(pipeline, "translate", fake_translate)
    monkeypatch.setattr(pipeline, "synthesize", lambda text, out_wav: None)

    result = pipeline.translate_audio("in.wav", str(tmp_path), src="es", tgt="en")

    assert result["source_text"] == "hola"
    assert result["translated_text"] == "hello"
    assert seen["args"] == ("hola", "es", "en")


def test_transcribe_serializes_concurrent_decodes(monkeypatch):
    """Parakeet is NeMo too: transcribe() mutates shared model state, and with
    cascade partials enabled /translate and /transcribe-partial can call it
    from two threads at once — the decode lock must serialize them."""

    class FakeConcurrentASR:
        def __init__(self):
            self.in_use = False

        def transcribe(self, paths):
            if self.in_use:
                raise AssertionError("transcribe() re-entered while in use")
            self.in_use = True
            try:
                time.sleep(0.05)
                return [SimpleNamespace(text=f"text for {paths[0]}")]
            finally:
                self.in_use = False

    fake = FakeConcurrentASR()
    monkeypatch.setattr(pipeline, "_get_asr", lambda: fake)

    results = {}
    errors = []

    def call(path):
        try:
            results[path] = pipeline.transcribe(path)
        except Exception as e:  # noqa: BLE001 — capture to fail the test explicitly
            errors.append(e)

    t1 = threading.Thread(target=call, args=("a.wav",))
    t2 = threading.Thread(target=call, args=("b.wav",))
    t1.start()
    t2.start()
    t1.join()
    t2.join()

    assert not errors, f"concurrent transcribe raised: {errors}"
    assert results["a.wav"] == "text for a.wav"
    assert results["b.wav"] == "text for b.wav"
