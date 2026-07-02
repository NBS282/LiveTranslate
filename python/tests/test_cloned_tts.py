"""Tests for the Pocket TTS cloned TTS wrapper."""

from unittest.mock import MagicMock
import numpy as np


# ── Packaging ───────────────────────────────────────────────────────────────


def test_pocket_tts_importable():
    """pocket-tts package MUST be importable."""
    import pocket_tts  # noqa: F401

    assert hasattr(pocket_tts, "TTSModel")


def test_chatterbox_not_required():
    """chatterbox/zipvoice MUST NOT be required by the engine anymore."""
    import importlib

    assert importlib.util.find_spec("zipvoice") is None


# ── Wrapper behaviour ───────────────────────────────────────────────────────


class TestClonedTTS:
    """Tests for the Pocket TTS wrapper."""

    def test_warmup_engine_returns_none(self):
        import lt_engine.cloned_tts as ctts

        ctts._model = MagicMock()
        assert ctts.warmup_engine() is None

    def test_warmup_calls_get_model_lazily(self, monkeypatch):
        import lt_engine.cloned_tts as ctts

        ctts._model = None
        called = False

        def fake_get_model():
            nonlocal called
            called = True
            return MagicMock()

        monkeypatch.setattr(ctts, "_get_model", fake_get_model)
        ctts.warmup_engine()
        assert called

    def test_synthesize_cloned_falls_back_without_profile(self, tmp_path, monkeypatch):
        import lt_engine.cloned_tts as ctts

        monkeypatch.setattr(ctts.vp, "exists", lambda: False)
        ctts.reset_voice_state()

        fallback_called = False

        def fake_fallback(text, out_wav):
            nonlocal fallback_called
            fallback_called = True
            import struct
            import wave

            with wave.open(out_wav, "wb") as wf:
                wf.setnchannels(1)
                wf.setsampwidth(2)
                wf.setframerate(22050)
                wf.writeframes(struct.pack("<h", 0))

        monkeypatch.setattr(ctts, "_fallback", fake_fallback)

        out = tmp_path / "test.wav"
        ctts.synthesize_cloned("Hello world", str(out))

        assert fallback_called
        assert out.exists()

    def test_synthesize_cloned_calls_generate_audio_with_state(self, tmp_path, monkeypatch):
        """With a voice state, synthesize_cloned calls model.generate_audio(state, text)."""
        import torch
        import lt_engine.cloned_tts as ctts

        mock_model = MagicMock()
        mock_model.generate_audio.return_value = torch.zeros(24000)
        mock_model.sample_rate = 24000

        monkeypatch.setattr(ctts, "_get_model", lambda: mock_model)
        monkeypatch.setattr(ctts, "get_voice_state", lambda: {"fake": "state"})

        out = tmp_path / "cloned.wav"
        ctts.synthesize_cloned("Hello world", str(out))

        assert out.exists()
        mock_model.generate_audio.assert_called_once()
        args = mock_model.generate_audio.call_args[0]
        assert args[0] == {"fake": "state"}
        assert args[1] == "Hello world"

    def test_synthesize_cloned_falls_back_on_failure(self, tmp_path, monkeypatch):
        import lt_engine.cloned_tts as ctts

        mock_model = MagicMock()
        mock_model.generate_audio.side_effect = RuntimeError("generation failed")
        monkeypatch.setattr(ctts, "_get_model", lambda: mock_model)
        monkeypatch.setattr(ctts, "get_voice_state", lambda: {"fake": "state"})

        fallback_called = False

        def fake_fallback(text, out_wav):
            nonlocal fallback_called
            fallback_called = True
            import struct
            import wave

            with wave.open(out_wav, "wb") as wf:
                wf.setnchannels(1)
                wf.setsampwidth(2)
                wf.setframerate(22050)
                wf.writeframes(struct.pack("<h", 0))

        monkeypatch.setattr(ctts, "_fallback", fake_fallback)

        out = tmp_path / "fail.wav"
        ctts.synthesize_cloned("Hello world", str(out))

        assert fallback_called
        assert out.exists()

    def test_reset_voice_state_clears_cache(self):
        import lt_engine.cloned_tts as ctts

        ctts._voice_state = {"some": "state"}
        ctts.reset_voice_state()
        assert ctts._voice_state is None

    def test_get_voice_state_returns_none_without_profile(self, monkeypatch):
        import lt_engine.cloned_tts as ctts

        monkeypatch.setattr(ctts.vp, "exists", lambda: False)
        ctts.reset_voice_state()
        assert ctts.get_voice_state() is None

    # ── Audio post-processing ────────────────────────────────────────────

    def test_normalize_audio_scales_to_target_db(self):
        import lt_engine.cloned_tts as ctts

        audio = np.ones(1000, dtype=np.float32) * 0.5
        result = ctts._normalize_audio(audio, target_db=-20.0, peak_limit=0.85)
        actual_rms = np.sqrt(np.mean(np.square(result)))
        expected_rms = 10 ** (-20.0 / 20)
        assert abs(actual_rms - expected_rms) < 1e-4

    def test_normalize_audio_applies_peak_limit(self):
        import lt_engine.cloned_tts as ctts

        audio = np.ones(1000, dtype=np.float32) * 5.0
        result = ctts._normalize_audio(audio, target_db=0.0, peak_limit=0.85)
        assert np.max(np.abs(result)) <= 0.85 + 1e-6

    def test_normalize_audio_handles_silence(self):
        import lt_engine.cloned_tts as ctts

        audio = np.zeros(1000, dtype=np.float32)
        result = ctts._normalize_audio(audio)
        np.testing.assert_array_equal(result, audio)

    def test_trim_preserves_audio_after_long_internal_pause(self):
        """Regression: multi-sentence output with a >1s inter-sentence pause must
        NOT be truncated at the gap — the trailing sentence must survive."""
        import lt_engine.cloned_tts as ctts

        sr = 24000
        tone = (0.3 * np.sin(2 * np.pi * 200 * np.arange(int(sr * 0.5)) / sr)).astype(np.float32)
        gap = np.zeros(int(sr * 1.5), dtype=np.float32)  # 1.5s pause between sentences
        audio = np.concatenate([tone, gap, tone])

        out = ctts._trim_tts_output(audio, sample_rate=sr)

        # Must keep both tones + the gap (~2.5s+), not cut at the 1.5s gap.
        assert len(out) / sr > 2.4

    def test_trim_removes_trailing_silence(self):
        import lt_engine.cloned_tts as ctts

        sr = 24000
        tone = (0.3 * np.sin(2 * np.pi * 200 * np.arange(int(sr * 0.5)) / sr)).astype(np.float32)
        tail = np.zeros(int(sr * 2.0), dtype=np.float32)
        audio = np.concatenate([tone, tail])

        out = ctts._trim_tts_output(audio, sample_rate=sr)

        # Trailing 2s of silence collapsed to a short pad; total well under input.
        assert len(out) / sr < 1.0

    def test_trim_prepends_leading_silence(self):
        import lt_engine.cloned_tts as ctts

        sr = 24000
        tone = (0.3 * np.sin(2 * np.pi * 200 * np.arange(int(sr * 0.5)) / sr)).astype(np.float32)
        out = ctts._trim_tts_output(tone, sample_rate=sr)

        lead = int(sr * 0.03)
        assert np.all(out[:lead] == 0)

    # ── WAV output format ────────────────────────────────────────────────

    def test_synthesize_cloned_output_is_24khz_mono_16bit(self, tmp_path, monkeypatch):
        import torch
        import wave
        import lt_engine.cloned_tts as ctts

        mock_model = MagicMock()
        signal = torch.sin(torch.linspace(0, 4 * 3.14159, 48000)) * 0.3
        mock_model.generate_audio.return_value = signal
        mock_model.sample_rate = 24000
        monkeypatch.setattr(ctts, "_get_model", lambda: mock_model)
        monkeypatch.setattr(ctts, "get_voice_state", lambda: {"fake": "state"})

        out = tmp_path / "fmt_check.wav"
        ctts.synthesize_cloned("Hello world", str(out))

        with wave.open(str(out), "rb") as wf:
            assert wf.getnchannels() == 1
            assert wf.getsampwidth() == 2
            assert wf.getframerate() == 24000


# ── Server voice-profile endpoints ───────────────────────────────────────────


class TestServerVoiceProfile:
    """Tests for server voice-profile endpoint behaviour."""

    def test_delete_voice_profile_calls_reset(self, monkeypatch):
        from lt_engine import server

        monkeypatch.setattr(server, "warmup", lambda: None)

        reset_called = False

        def fake_reset():
            nonlocal reset_called
            reset_called = True

        monkeypatch.setattr(server, "reset_voice_state", fake_reset)
        monkeypatch.setattr(server.vp, "delete", lambda: None)

        from fastapi.testclient import TestClient

        with TestClient(server.app) as client:
            r = client.delete("/voice-profile")
            assert r.status_code == 200
            assert r.json() == {"exists": False}
            assert reset_called

    def test_upload_voice_profile_accepts_raw_body(self, monkeypatch):
        """Upload MUST accept raw WAV bytes in the body (no multipart form).

        Sending the audio as the raw request body removes the python-multipart
        dependency and chunked-encoding edge cases that broke uploads on a fresh
        install. The bytes MUST reach vp.save unchanged.
        """
        from lt_engine import server

        monkeypatch.setattr(server, "warmup", lambda: None)
        monkeypatch.setattr(server, "cloning_available", lambda: True)
        monkeypatch.setattr(server, "reset_voice_state", lambda: None)
        monkeypatch.setattr(server, "export_voice_state", lambda: None)

        saved = {}
        monkeypatch.setattr(server.vp, "save", lambda data: saved.update(bytes=data))

        from fastapi.testclient import TestClient

        payload = b"not-a-real-wav-but-non-empty"
        with TestClient(server.app) as client:
            r = client.post(
                "/voice-profile",
                content=payload,
                headers={"Content-Type": "audio/wav"},
            )
            assert r.status_code == 200
            assert r.json() == {"exists": True}
            assert saved["bytes"] == payload

    def test_upload_voice_profile_rejects_empty_body(self, monkeypatch):
        from lt_engine import server

        monkeypatch.setattr(server, "warmup", lambda: None)
        monkeypatch.setattr(server, "cloning_available", lambda: True)

        from fastapi.testclient import TestClient

        with TestClient(server.app) as client:
            r = client.post(
                "/voice-profile",
                content=b"",
                headers={"Content-Type": "audio/wav"},
            )
            assert r.status_code == 400
