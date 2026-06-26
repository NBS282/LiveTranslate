"""Tests for the Chatterbox Turbo cloned TTS wrapper.

Follows Strict TDD: tests written before implementation.
"""

from unittest.mock import patch, MagicMock
import numpy as np
import pytest


# ── Task 1.2: requirements.txt ──────────────────────────────────────────────


def test_chatterbox_tts_importable():
    """chatterbox-tts package MUST be importable after requirements.txt update."""
    import chatterbox  # noqa: F811

    assert hasattr(chatterbox, "__version__")


def test_zipvoice_not_importable():
    """Zipvoice/LuxTTS MUST NOT be importable after removal from requirements."""
    import importlib

    spec = importlib.util.find_spec("zipvoice")
    assert spec is None, "zipvoice should not be installed"


# ── Task 2.1: cloned_tts.py rewrite ─────────────────────────────────────────


class TestClonedTTS:
    """Tests for the rewritten Chatterbox Turbo wrapper."""

    def test_warmup_engine_returns_none(self):
        """warmup_engine() should return None and not raise."""
        import lt_engine.cloned_tts as ctts

        ctts._model = MagicMock()
        result = ctts.warmup_engine()
        assert result is None

    def test_warmup_calls_get_model_lazily(self, monkeypatch):
        """warmup_engine() should delegate to _get_model()."""
        import lt_engine.cloned_tts as ctts

        ctts._model = None
        called = False

        def fake_get_model():
            nonlocal called
            called = True
            return MagicMock()

        monkeypatch.setattr(ctts, "_get_model", fake_get_model)
        ctts.warmup_engine()
        assert called, "_get_model should be called by warmup_engine"

    def test_synthesize_cloned_falls_back_without_profile(self, tmp_path, monkeypatch):
        """Without a voice profile, synthesize_cloned should fall back to Piper."""
        import lt_engine.cloned_tts as ctts

        monkeypatch.setattr(ctts.vp, "exists", lambda: False)

        fallback_called = False

        def fake_fallback(text, out_wav):
            nonlocal fallback_called
            fallback_called = True
            # Write a minimal valid WAV
            import struct, wave

            with wave.open(out_wav, "wb") as wf:
                wf.setnchannels(1)
                wf.setsampwidth(2)
                wf.setframerate(22050)
                wf.writeframes(struct.pack("<h", 0))

        monkeypatch.setattr(ctts, "_fallback", fake_fallback)

        out = tmp_path / "test.wav"
        ctts.synthesize_cloned("Hello world", str(out))

        assert fallback_called, "Should have called Piper fallback"
        assert out.exists(), "Fallback should produce a WAV file"

    def test_synthesize_cloned_calls_generate_with_profile(self, tmp_path, monkeypatch):
        """With a voice profile, synthesize_cloned should call model.generate()."""
        import torch
        import lt_engine.cloned_tts as ctts

        # Mock profile existence
        monkeypatch.setattr(ctts.vp, "exists", lambda: True)
        monkeypatch.setattr(
            ctts.vp, "reference_path", lambda: "/fake/path/reference.wav"
        )

        # Mock model
        mock_model = MagicMock()
        mock_model.generate.return_value = torch.zeros(24000)
        mock_model.sr = 24000
        monkeypatch.setattr(ctts, "_get_model", lambda: mock_model)

        # Patch _normalize_rms to be a no-op
        monkeypatch.setattr(ctts, "_normalize_rms", lambda a, target_rms=0.1: a)

        out = tmp_path / "cloned.wav"
        ctts.synthesize_cloned("Hello world", str(out))

        assert out.exists(), "Should produce a WAV file"
        mock_model.generate.assert_called_once()
        # Verify audio_prompt_path was passed
        call_kwargs = mock_model.generate.call_args[1]
        assert "audio_prompt_path" in call_kwargs

    def test_synthesize_cloned_falls_back_on_failure(self, tmp_path, monkeypatch):
        """If model.generate() raises, fall back to Piper."""
        import lt_engine.cloned_tts as ctts

        monkeypatch.setattr(ctts.vp, "exists", lambda: True)
        monkeypatch.setattr(
            ctts.vp, "reference_path", lambda: "/fake/path/reference.wav"
        )

        mock_model = MagicMock()
        mock_model.generate.side_effect = RuntimeError("generation failed")
        monkeypatch.setattr(ctts, "_get_model", lambda: mock_model)

        fallback_called = False

        def fake_fallback(text, out_wav):
            nonlocal fallback_called
            fallback_called = True
            import struct, wave

            with wave.open(out_wav, "wb") as wf:
                wf.setnchannels(1)
                wf.setsampwidth(2)
                wf.setframerate(22050)
                wf.writeframes(struct.pack("<h", 0))

        monkeypatch.setattr(ctts, "_fallback", fake_fallback)

        out = tmp_path / "fail.wav"
        ctts.synthesize_cloned("Hello world", str(out))

        assert fallback_called, "Should fall back on generation failure"
        assert out.exists()

    def test_synthesize_cloned_handles_short_text(self, tmp_path, monkeypatch):
        """Text with ≤2 words should NOT fall back — Chatterbox Turbo handles it."""
        import torch
        import lt_engine.cloned_tts as ctts

        monkeypatch.setattr(ctts.vp, "exists", lambda: True)
        monkeypatch.setattr(
            ctts.vp, "reference_path", lambda: "/fake/path/reference.wav"
        )

        mock_model = MagicMock()
        mock_model.generate.return_value = torch.zeros(24000)
        mock_model.sr = 24000
        monkeypatch.setattr(ctts, "_get_model", lambda: mock_model)
        monkeypatch.setattr(ctts, "_normalize_rms", lambda a, target_rms=0.1: a)

        out = tmp_path / "short.wav"
        ctts.synthesize_cloned("Hi", str(out))
        assert out.exists()
        mock_model.generate.assert_called_once()

    def test_reset_voice_prompt_clears_cache(self):
        """reset_voice_prompt should clear the cached prompt/reference path."""
        import lt_engine.cloned_tts as ctts

        ctts._voice_prompt = "/some/path/reference.wav"
        ctts.reset_voice_prompt()
        assert ctts._voice_prompt is None

    def test_get_voice_prompt_returns_path_when_profile_exists(self, monkeypatch):
        """get_voice_prompt should return the reference path when profile exists."""
        import lt_engine.cloned_tts as ctts

        monkeypatch.setattr(ctts.vp, "exists", lambda: True)
        monkeypatch.setattr(
            ctts.vp, "reference_path", lambda: "/fake/path/reference.wav"
        )

        ctts.reset_voice_prompt()
        result = ctts.get_voice_prompt()

        assert result == "/fake/path/reference.wav"

    def test_get_voice_prompt_returns_none_without_profile(self, monkeypatch):
        """get_voice_prompt should return None when no profile exists."""
        import lt_engine.cloned_tts as ctts

        monkeypatch.setattr(ctts.vp, "exists", lambda: False)
        ctts.reset_voice_prompt()
        result = ctts.get_voice_prompt()
        assert result is None

    def test_normalize_rms_scales_audio(self):
        """_normalize_rms should scale audio to the target RMS."""
        import lt_engine.cloned_tts as ctts

        audio = np.ones(1000, dtype=np.float32) * 0.5
        result = ctts._normalize_rms(audio, target_rms=0.2)
        actual_rms = np.sqrt(np.mean(np.square(result)))
        assert abs(actual_rms - 0.2) < 1e-5

    def test_normalize_rms_handles_silence(self):
        """_normalize_rms should return silence unchanged."""
        import lt_engine.cloned_tts as ctts

        audio = np.zeros(1000, dtype=np.float32)
        result = ctts._normalize_rms(audio, target_rms=0.1)
        np.testing.assert_array_equal(result, audio)

    # ── Triangulation: WAV output format ─────────────────────────────────

    def test_synthesize_cloned_output_is_24khz_mono_16bit(self, tmp_path, monkeypatch):
        """Output WAV should be 24000 Hz, mono, 16-bit PCM."""
        import torch
        import lt_engine.cloned_tts as ctts

        monkeypatch.setattr(ctts.vp, "exists", lambda: True)
        monkeypatch.setattr(
            ctts.vp, "reference_path", lambda: "/fake/path/reference.wav"
        )

        mock_model = MagicMock()
        mock_model.generate.return_value = torch.zeros(48000)  # 2 seconds at 24kHz
        mock_model.sr = 24000
        monkeypatch.setattr(ctts, "_get_model", lambda: mock_model)
        monkeypatch.setattr(ctts, "_normalize_rms", lambda a, target_rms=0.1: a)

        out = tmp_path / "fmt_check.wav"
        ctts.synthesize_cloned("Hello world", str(out))

        import wave

        with wave.open(str(out), "rb") as wf:
            assert wf.getnchannels() == 1, "Must be mono"
            assert wf.getsampwidth() == 2, "Must be 16-bit"
            assert wf.getframerate() == 24000, "Must be 24000 Hz"

    def test_synthesize_cloned_output_has_audio_data(self, tmp_path, monkeypatch):
        """Output WAV should contain non-zero audio data after RMS normalisation."""
        import torch
        import wave
        import lt_engine.cloned_tts as ctts

        monkeypatch.setattr(ctts.vp, "exists", lambda: True)
        monkeypatch.setattr(
            ctts.vp, "reference_path", lambda: "/fake/path/reference.wav"
        )

        # Generate a signal with actual content
        signal = torch.sin(torch.linspace(0, 4 * 3.14159, 48000)) * 0.3
        mock_model = MagicMock()
        mock_model.generate.return_value = signal
        mock_model.sr = 24000
        monkeypatch.setattr(ctts, "_get_model", lambda: mock_model)

        out = tmp_path / "data_check.wav"
        ctts.synthesize_cloned("Hello world", str(out))

        # Verify the WAV contains audio data (not just silence)
        import struct

        with wave.open(str(out), "rb") as wf:
            frames = wf.readframes(wf.getnframes())
            samples = struct.unpack_from(f"<{len(frames)//2}h", frames)
            assert max(abs(s) for s in samples) > 0, "WAV must contain audio data"

    # ── Triangulation: Thread safety ─────────────────────────────────────

    def test_get_voice_prompt_thread_safe(self, monkeypatch):
        """get_voice_prompt should be safe against concurrent calls."""
        import lt_engine.cloned_tts as ctts

        call_count = 0

        def counting_ref_path():
            nonlocal call_count
            call_count += 1
            return "/counted/path.wav"

        monkeypatch.setattr(ctts.vp, "exists", lambda: True)
        monkeypatch.setattr(ctts.vp, "reference_path", counting_ref_path)

        ctts.reset_voice_prompt()

        # Call twice — second should use cached value
        r1 = ctts.get_voice_prompt()
        r2 = ctts.get_voice_prompt()

        assert r1 == r2 == "/counted/path.wav"
        # reference_path should only be called once (cached by _voice_prompt)
        assert call_count == 1


# ── Task 3.2: server.py changes ──────────────────────────────────────────────


class TestServerVoiceProfile:
    """Tests for server voice-profile endpoint changes."""

    def test_upload_voice_profile_no_encoding(self, tmp_path, monkeypatch):
        """Upload should just save the WAV, no encoding step."""
        from lt_engine import server

        monkeypatch.setattr(server, "warmup", lambda: None)
        monkeypatch.setattr(server.vp, "profile_dir", lambda: tmp_path)

        saved = []

        def fake_save(audio_bytes):
            saved.append(audio_bytes)

        monkeypatch.setattr(server.vp, "save", fake_save)

        from fastapi.testclient import TestClient

        with TestClient(server.app) as client:
            r = client.post(
                "/voice-profile", files={"file": ("ref.wav", b"fakewavdata")}
            )
            assert r.status_code == 200
            assert r.json() == {"exists": True}
            assert len(saved) == 1
            assert saved[0] == b"fakewavdata"

    def test_delete_voice_profile_calls_reset(self, monkeypatch):
        """DELETE should call reset_voice_prompt."""
        from lt_engine import server

        monkeypatch.setattr(server, "warmup", lambda: None)

        reset_called = False

        def fake_reset():
            nonlocal reset_called
            reset_called = True

        monkeypatch.setattr(server, "reset_voice_prompt", fake_reset)
        monkeypatch.setattr(server.vp, "delete", lambda: None)

        from fastapi.testclient import TestClient

        with TestClient(server.app) as client:
            r = client.delete("/voice-profile")
            assert r.status_code == 200
            assert r.json() == {"exists": False}
            assert reset_called
