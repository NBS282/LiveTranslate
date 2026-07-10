"""Orchestration contract of the model pre-download step (setup phase 5).

The real downloads are mocked — these tests pin WHAT downloads, that
required failures abort, and that the optional Pocket TTS never does.
"""
import pytest

import lt_engine.setup_models as sm


def _patch_downloads(monkeypatch, calls):
    monkeypatch.setattr(sm, "download_canary", lambda: calls.append("canary"))
    monkeypatch.setattr(sm, "download_parakeet", lambda: calls.append("parakeet"))
    monkeypatch.setattr(sm, "download_marian", lambda: calls.append("marian"))
    monkeypatch.setattr(sm, "download_pocket_tts", lambda: calls.append("pocket"))
    monkeypatch.setattr(sm, "_patch_windows_symlinks", lambda: None)


def test_download_all_downloads_every_model(monkeypatch):
    calls = []
    _patch_downloads(monkeypatch, calls)

    sm.download_all(max_workers=2)

    assert set(calls) == {"canary", "parakeet", "marian", "pocket"}


def test_optional_pocket_tts_failure_does_not_abort(monkeypatch):
    calls = []
    _patch_downloads(monkeypatch, calls)

    def boom():
        raise RuntimeError("401 gated repo")

    monkeypatch.setattr(sm, "download_pocket_tts", boom)

    sm.download_all(max_workers=2)  # must not raise

    assert set(calls) == {"canary", "parakeet", "marian"}


def test_required_model_failure_raises_after_all_settle(monkeypatch):
    calls = []
    _patch_downloads(monkeypatch, calls)

    def boom():
        raise RuntimeError("network down")

    monkeypatch.setattr(sm, "download_canary", boom)

    with pytest.raises(RuntimeError, match="Canary"):
        sm.download_all(max_workers=2)

    # The other downloads still ran — a partial cache shortens the retry.
    assert {"parakeet", "marian"} <= set(calls)


def test_main_returns_nonzero_on_required_failure(monkeypatch):
    calls = []
    _patch_downloads(monkeypatch, calls)

    def boom():
        raise RuntimeError("network down")

    monkeypatch.setattr(sm, "download_parakeet", boom)

    assert sm.main() == 1


def test_main_returns_zero_on_success(monkeypatch):
    calls = []
    _patch_downloads(monkeypatch, calls)

    assert sm.main() == 0


def test_progress_pct_scales_and_clamps():
    assert sm.progress_pct(0) == 0
    assert sm.progress_pct(-5) == 0
    assert sm.progress_pct(sm.TOTAL_DOWNLOAD_BYTES // 2) == 50
    # Clamped below 100: only actual completion prints PROGRESS:100.
    assert sm.progress_pct(sm.TOTAL_DOWNLOAD_BYTES) == 99
    assert sm.progress_pct(sm.TOTAL_DOWNLOAD_BYTES * 2) == 99
    assert sm.progress_pct(10, total=0) == 0
