# Modular Engine — Spike Notes (Task 0)

Verified on the dev machine (Windows, **CPU-only**, no GPU touched). All three stages work.

## Environment

```bash
uv venv -p 3.11 .venv-engine
source .venv-engine/Scripts/activate
uv pip install torch --index-url https://download.pytorch.org/whl/cpu   # CPU build, stable
uv pip install "nemo_toolkit[asr]" transformers sentencepiece piper-tts soundfile
python -m piper.download_voices en_US-lessac-medium                     # downloads en_US-lessac-medium.onnx (+ .onnx.json)
```
- Python 3.11 (NeMo not ready for 3.13).
- `nemo_toolkit[asr]` installs cleanly on Windows CPU. ✅ (the main risk — cleared)
- Optional warning: no ffmpeg (pydub) — not needed for wav input.

## Verified APIs

### STT — Parakeet (`nvidia/parakeet-tdt-0.6b-v3`)
```python
from nemo.collections.asr.models import ASRModel
asr = ASRModel.from_pretrained("nvidia/parakeet-tdt-0.6b-v3", map_location="cpu")
out = asr.transcribe([audio_path])     # -> list[Hypothesis]
text = out[0].text                      # Hypothesis has .text  (getattr(out[0], "text", out[0]) is safe)
```
- Spanish transcription quality: excellent ("Mi nombre es Nicolás, soy de Las Piedras, vivo en Canelones, estudio Ingeniería en Sistemas").
- Model checkpoint ~2.5 GB (downloaded once, cached in HF hub).

### MT — NLLB-200 (`facebook/nllb-200-distilled-600M`)
```python
from transformers import AutoTokenizer, AutoModelForSeq2SeqLM
tok = AutoTokenizer.from_pretrained(name)
model = AutoModelForSeq2SeqLM.from_pretrained(name)
tok.src_lang = "spa_Latn"
inputs = tok(text, return_tensors="pt")
gen = model.generate(**inputs, forced_bos_token_id=tok.convert_tokens_to_ids("eng_Latn"), max_length=512)
english = tok.batch_decode(gen, skip_special_tokens=True)[0]
```
- Translation correct. Model ~2.46 GB (cached).

### TTS — Piper (`en_US-lessac-medium`)
```python
import wave
from piper import PiperVoice
voice = PiperVoice.load("en_US-lessac-medium.onnx")     # voice file in repo root after download_voices
with wave.open(out_wav, "wb") as wf:
    voice.synthesize_wav(text, wf)                       # NOTE: synthesize_wav (writes header); synthesize() returns chunks and does NOT
```
- `synthesize()` (no `_wav`) fails with `wave.Error: # channels not specified` — must use **`synthesize_wav`**.

## CPU timings (dev machine)

| Stage | Load (once) | Run |
|---|---|---|
| Parakeet STT | ~15 s (cached) / ~140 s first (download) | **0.5 s** (short clip) |
| NLLB MT | ~5 s (cached) | **1.6 s** |
| Piper TTS | ~1 s | sub-second |

→ Per-utterance steady-state ≈ **~2 s on CPU** after models are loaded. Viable for near-real-time by phrase. **No GPU used.**

## Implications for the orchestrator (Task 1)
- Load each model ONCE (not per call). For file→file CLI it's per-invocation; for streaming, keep loaded.
- Use the Python APIs above (not the Piper CLI) — `synthesize_wav` confirmed.
- Piper voice path needs to be resolvable; expose via env (`PIPER_VOICE`) with a sane default; ship/download the `.onnx` during setup.
