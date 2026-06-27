from fastapi.testclient import TestClient
import lt_engine.server as server


def test_health_ok(monkeypatch):
    monkeypatch.setattr(server, "warmup", lambda: None)
    with TestClient(server.app) as client:
        r = client.get("/health")
        assert r.status_code == 200
        assert r.json() == {"ready": True}


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
