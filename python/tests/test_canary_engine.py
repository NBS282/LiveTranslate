"""Canary AST engine routing. The model itself is mocked (3.5 GB download)."""
import threading
import time
from types import SimpleNamespace

import lt_engine.pipeline as pipeline


class FakeCanary:
    def __init__(self, text=" Hello world."):
        self.text = text
        self.calls = []

    def transcribe(self, paths, **kwargs):
        self.calls.append((paths, kwargs))
        return [SimpleNamespace(text=self.text)]


def test_decode_ast_calls_canary_ast(monkeypatch):
    fake = FakeCanary()
    monkeypatch.setattr(pipeline, "_get_canary", lambda: fake)

    out = pipeline._decode_ast("in.wav")

    assert out == "Hello world."
    paths, kwargs = fake.calls[0]
    assert paths == ["in.wav"]
    assert kwargs["task"] == "ast"
    assert kwargs["source_lang"] == "es"
    assert kwargs["target_lang"] == "en"
    assert kwargs["pnc"] == "yes"


def test_decode_ast_passes_selected_language_pair(monkeypatch):
    """Canary 1B Flash supports EN<->DE/ES/FR; the pair must reach the model
    instead of being hardcoded to es->en."""
    fake = FakeCanary()
    monkeypatch.setattr(pipeline, "_get_canary", lambda: fake)

    pipeline._decode_ast("in.wav", source_lang="fr", target_lang="en")

    _, kwargs = fake.calls[0]
    assert kwargs["source_lang"] == "fr"
    assert kwargs["target_lang"] == "en"


def test_speech_translate_forwards_language_pair(tmp_path, monkeypatch):
    import numpy as np
    import soundfile as sf

    audio = np.sin(np.linspace(0, 400 * np.pi, 32_000)).astype("float32") * 0.5
    src = tmp_path / "clip.wav"
    sf.write(str(src), audio, 16_000)

    seen = {}

    def fake_decode(path, source_lang="es", target_lang="en"):
        seen["pair"] = (source_lang, target_lang)
        return "Hallo."

    monkeypatch.setattr(pipeline, "_decode_ast", fake_decode)

    out = pipeline.speech_translate(str(src), source_lang="en", target_lang="de")

    assert out == "Hallo."
    assert seen["pair"] == ("en", "de")


def test_speech_translate_rejects_unsupported_pair(tmp_path):
    import pytest

    with pytest.raises(ValueError, match="unsupported language pair"):
        pipeline.speech_translate("in.wav", source_lang="es", target_lang="de")


def test_validate_language_pair_normalizes_case_and_whitespace():
    assert pipeline.validate_language_pair(" ES ", "en") == ("es", "en")


def test_translate_audio_canary_forwards_pair(monkeypatch, tmp_path):
    monkeypatch.setattr(pipeline, "translation_engine", lambda: "canary")
    seen = {}

    def fake_speech_translate(path, allow_bisect=True, source_lang="es", target_lang="en"):
        seen["pair"] = (source_lang, target_lang)
        return "Bonjour."

    monkeypatch.setattr(pipeline, "speech_translate", fake_speech_translate)
    monkeypatch.setattr(pipeline, "synthesize", lambda text, out_wav: None)

    result = pipeline.translate_audio("in.wav", str(tmp_path), src="en", tgt="fr")

    assert result["translated_text"] == "Bonjour."
    assert seen["pair"] == ("en", "fr")


def test_translate_audio_routes_to_canary(monkeypatch, tmp_path):
    monkeypatch.setattr(pipeline, "translation_engine", lambda: "canary")
    monkeypatch.setattr(pipeline, "speech_translate", lambda p, **kw: "Hello there.")
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
    monkeypatch.setattr(pipeline, "speech_translate", lambda p, **kw: "   ")

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


def test_warmup_canary_engine_skips_parakeet_and_marian(monkeypatch):
    calls = []
    monkeypatch.setattr(pipeline, "translation_engine", lambda: "canary")
    monkeypatch.setattr(pipeline, "_get_canary", lambda: calls.append("canary"))
    monkeypatch.setattr(pipeline, "_get_asr", lambda: calls.append("asr"))
    monkeypatch.setattr(pipeline, "_get_mt", lambda: calls.append("mt"))
    monkeypatch.setattr(pipeline, "_get_piper", lambda: calls.append("piper"))
    import lt_engine.cloned_tts as ct
    monkeypatch.setattr(ct, "warmup_engine", lambda: None)

    pipeline.warmup()

    assert "canary" in calls and "piper" in calls
    assert "asr" not in calls and "mt" not in calls


def test_warmup_legacy_engine_loads_parakeet_and_marian(monkeypatch):
    calls = []
    monkeypatch.setattr(pipeline, "translation_engine", lambda: "legacy")
    monkeypatch.setattr(pipeline, "_get_canary", lambda: calls.append("canary"))
    monkeypatch.setattr(pipeline, "_get_asr", lambda: calls.append("asr"))
    monkeypatch.setattr(pipeline, "_get_mt", lambda: calls.append("mt"))
    monkeypatch.setattr(pipeline, "_get_piper", lambda: calls.append("piper"))
    import lt_engine.cloned_tts as ct
    monkeypatch.setattr(ct, "warmup_engine", lambda: None)

    pipeline.warmup()

    assert "asr" in calls and "mt" in calls
    assert "canary" not in calls


def test_decode_ast_serializes_concurrent_decodes(monkeypatch):
    """/translate and /transcribe-partial can call speech_translate from two
    FastAPI threadpool threads at once. NeMo's transcribe() mutates shared
    model state, so overlapping calls must never run concurrently — the
    module-level _decode_lock must serialize them."""

    class FakeConcurrentCanary:
        def __init__(self):
            self.in_use = False

        def transcribe(self, paths, **kwargs):
            if self.in_use:
                raise AssertionError("transcribe() re-entered while in use")
            self.in_use = True
            try:
                time.sleep(0.05)
                return [SimpleNamespace(text=f"result for {paths[0]}")]
            finally:
                self.in_use = False

    fake = FakeConcurrentCanary()
    monkeypatch.setattr(pipeline, "_get_canary", lambda: fake)

    results = {}
    errors = []

    def call(path):
        try:
            results[path] = pipeline._decode_ast(path)
        except Exception as e:  # noqa: BLE001 — capture to fail the test explicitly
            errors.append(e)

    t1 = threading.Thread(target=call, args=("a.wav",))
    t2 = threading.Thread(target=call, args=("b.wav",))
    t1.start()
    t2.start()
    t1.join()
    t2.join()

    assert not errors, f"concurrent decode raised: {errors}"
    assert results["a.wav"] == "result for a.wav"
    assert results["b.wav"] == "result for b.wav"


def test_decode_ast_strips_special_tokens(monkeypatch):
    """Near-silent segments make the decoder emit raw special tokens like
    <|endoftext|>) — regression: that garbage was displayed and synthesized."""
    fake = FakeCanary(text=" <|endoftext|>) ")
    monkeypatch.setattr(pipeline, "_get_canary", lambda: fake)

    assert pipeline._decode_ast("in.wav") == ""


def test_decode_ast_strips_tokens_but_keeps_real_text(monkeypatch):
    fake = FakeCanary(text="Hello there.<|endoftext|>")
    monkeypatch.setattr(pipeline, "_get_canary", lambda: fake)

    assert pipeline._decode_ast("in.wav") == "Hello there."


def test_speech_translate_normalizes_quiet_audio(tmp_path, monkeypatch):
    """Mic captures peak ~0.1; Canary collapses on quiet audio. The decode
    input must be peak-normalized (~0.9)."""
    import numpy as np
    import soundfile as sf

    quiet = np.sin(np.linspace(0, 400 * np.pi, 32_000)).astype("float32") * 0.05
    src = tmp_path / "quiet.wav"
    sf.write(str(src), quiet, 16_000)

    seen = {}

    def fake_decode(path, **kw):
        audio, _ = sf.read(path, dtype="float32")
        seen["peak"] = float(abs(audio).max())
        return "Hello."

    monkeypatch.setattr(pipeline, "_decode_ast", fake_decode)

    assert pipeline.speech_translate(str(src)) == "Hello."
    assert 0.85 <= seen["peak"] <= 0.95


def test_speech_translate_bisects_on_empty_decode(tmp_path, monkeypatch):
    """A code-switched span can make Canary AST emit nothing for a whole 8s
    segment. Regression: split in halves and recover what decodes."""
    import numpy as np
    import soundfile as sf

    audio = np.sin(np.linspace(0, 4000 * np.pi, 128_000)).astype("float32") * 0.5
    src = tmp_path / "full.wav"
    sf.write(str(src), audio, 16_000)

    def fake_decode(path, **kw):
        import soundfile as sf2

        a, sr = sf2.read(path, dtype="float32")
        dur = len(a) / sr
        if dur > 6:
            return ""  # full 8s collapses
        if dur > 3:
            return "First half." if fake_decode.calls == 1 else "Second half."
        return ""

    calls = {"n": 0}

    def counting_decode(path, **kw):
        calls["n"] += 1
        fake_decode.calls = calls["n"] - 1  # 0 = full, 1 = first half, 2 = second
        return fake_decode(path)

    monkeypatch.setattr(pipeline, "_decode_ast", counting_decode)

    assert pipeline.speech_translate(str(src)) == "First half. Second half."


def test_speech_translate_no_bisect_for_short_clips(tmp_path, monkeypatch):
    """Breath tails (<4s) that decode empty must NOT trigger extra decodes."""
    import numpy as np
    import soundfile as sf

    audio = np.zeros(16_000, dtype="float32")
    src = tmp_path / "short.wav"
    sf.write(str(src), audio, 16_000)

    calls = {"n": 0}

    def fake_decode(path, **kw):
        calls["n"] += 1
        return ""

    monkeypatch.setattr(pipeline, "_decode_ast", fake_decode)

    assert pipeline.speech_translate(str(src)) == ""
    assert calls["n"] == 1


def test_speech_translate_partials_never_bisect(tmp_path, monkeypatch):
    """The partial hot path must do exactly one decode even on empty output."""
    import numpy as np
    import soundfile as sf

    audio = np.sin(np.linspace(0, 4000 * np.pi, 128_000)).astype("float32") * 0.5
    src = tmp_path / "partial.wav"
    sf.write(str(src), audio, 16_000)

    calls = {"n": 0}

    def fake_decode(path, **kw):
        calls["n"] += 1
        return ""

    monkeypatch.setattr(pipeline, "_decode_ast", fake_decode)

    assert pipeline.speech_translate(str(src), allow_bisect=False) == ""
    assert calls["n"] == 1
