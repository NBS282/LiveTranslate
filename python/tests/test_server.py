from fastapi.testclient import TestClient
import lt_engine.server as server


def test_health_ok(monkeypatch):
    monkeypatch.setattr(server, "warmup", lambda: None)
    monkeypatch.setattr(server, "cloning_available", lambda: True)
    with TestClient(server.app) as client:
        r = client.get("/health")
        assert r.status_code == 200
        assert r.json() == {"ready": True, "cloning_available": True}


def test_health_reports_cloning_unavailable(monkeypatch):
    monkeypatch.setattr(server, "warmup", lambda: None)
    monkeypatch.setattr(server, "cloning_available", lambda: False)
    with TestClient(server.app) as client:
        r = client.get("/health")
        assert r.status_code == 200
        assert r.json()["cloning_available"] is False


def test_voice_profile_upload_503_when_cloning_unavailable(monkeypatch):
    monkeypatch.setattr(server, "warmup", lambda: None)
    monkeypatch.setattr(server, "cloning_available", lambda: False)
    monkeypatch.setattr(server, "cloning_error", lambda: "401 gated repo")
    with TestClient(server.app) as client:
        r = client.post("/voice-profile", content=b"RIFFfakewav")
        assert r.status_code == 503
        assert "401 gated repo" in r.json()["detail"]


def test_translate_calls_engine(monkeypatch, tmp_path):
    monkeypatch.setattr(server, "warmup", lambda: None)
    captured = {}

    def fake(input_path, out_dir, src, tgt, use_cloned_voice=False):
        captured.update(
            input_path=input_path, src=src, tgt=tgt, use_cloned_voice=use_cloned_voice
        )
        return {"output_wav": "out.wav", "source_text": "hola", "translated_text": "hello"}

    monkeypatch.setattr(server, "translate_audio", fake)
    f = tmp_path / "in.wav"
    f.write_bytes(b"x")
    with TestClient(server.app) as client:
        r = client.post(
            "/translate",
            json={"input_path": str(f), "out_dir": str(tmp_path), "use_cloned_voice": True},
        )
        assert r.status_code == 200
        assert r.json()["translated_text"] == "hello"
        assert captured["src"] == "es"
        assert captured["use_cloned_voice"] is True


def test_translate_missing_file_400(monkeypatch):
    monkeypatch.setattr(server, "warmup", lambda: None)
    with TestClient(server.app) as client:
        r = client.post("/translate", json={"input_path": "/no/such.wav", "out_dir": "."})
        assert r.status_code == 400


def test_transcribe_partial_returns_text(monkeypatch, tmp_path):
    monkeypatch.setattr(server, "warmup", lambda: None)
    monkeypatch.setattr(server, "speech_translate", lambda p, **kw: "Partial text")
    f = tmp_path / "chunk.wav"
    f.write_bytes(b"x")
    with TestClient(server.app) as client:
        r = client.post("/transcribe-partial", json={"input_path": str(f)})
        assert r.status_code == 200
        assert r.json() == {"text": "Partial text"}


def test_transcribe_partial_missing_file_400(monkeypatch):
    monkeypatch.setattr(server, "warmup", lambda: None)
    with TestClient(server.app) as client:
        r = client.post("/transcribe-partial", json={"input_path": "/no/such.wav"})
        assert r.status_code == 400


def test_transcribe_partial_returns_empty_under_legacy_engine(monkeypatch, tmp_path):
    """Under LT_TRANSLATION_ENGINE=legacy, /transcribe-partial must not lazy-load
    the 3.5GB Canary model — that would break the rollback guarantee. It must
    short-circuit to the empty-text signal without calling speech_translate."""
    monkeypatch.setattr(server, "warmup", lambda: None)
    monkeypatch.setattr(server, "translation_engine", lambda: "legacy")

    def must_not_be_called(path):
        raise AssertionError("must not be called")

    monkeypatch.setattr(server, "speech_translate", must_not_be_called)
    f = tmp_path / "chunk.wav"
    f.write_bytes(b"x")
    with TestClient(server.app) as client:
        r = client.post("/transcribe-partial", json={"input_path": str(f)})
        assert r.status_code == 200
        assert r.json() == {"text": ""}


def test_transcribe_partial_decode_failure_500(monkeypatch, tmp_path):
    monkeypatch.setattr(server, "warmup", lambda: None)

    def boom(path, **kw):
        raise RuntimeError("decoder exploded")

    monkeypatch.setattr(server, "speech_translate", boom)
    f = tmp_path / "chunk.wav"
    f.write_bytes(b"x")
    with TestClient(server.app) as client:
        r = client.post("/transcribe-partial", json={"input_path": str(f)})
        assert r.status_code == 500
        assert "decoder exploded" in r.json()["detail"]
