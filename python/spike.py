"""Task 0 spike: validate the modular CPU pipeline (Parakeet -> NLLB -> Piper).

Run from the repo root with the engine venv active:
    python python/spike.py [path-to-spanish-audio.wav]

Each stage is wrapped in try/except so one failure does not block the others.
The goal is to DISCOVER the real APIs (return shapes, call signatures) and record
them in python/SPIKE_NOTES.md. CPU-only — this must not touch the GPU.
"""
import sys
import time
import traceback

if len(sys.argv) < 2:
    sys.exit("usage: python spike.py <path-to-spanish-audio.wav>")
AUDIO = sys.argv[1]
PIPER_VOICE = "en_US-lessac-medium"  # download first: python -m piper.download_voices en_US-lessac-medium

print(f"Audio sample: {AUDIO}\n")

# ---------------------------------------------------------------- STT: Parakeet
print("=== STT: Parakeet (nvidia/parakeet-tdt-0.6b-v3) ===")
src_text = None
try:
    t0 = time.time()
    from nemo.collections.asr.models import ASRModel
    asr = ASRModel.from_pretrained("nvidia/parakeet-tdt-0.6b-v3", map_location="cpu")
    print(f"  model loaded in {time.time()-t0:.1f}s")
    t1 = time.time()
    out = asr.transcribe([AUDIO])
    print(f"  transcribe() in {time.time()-t1:.1f}s")
    print("  return type:", type(out))
    print("  item[0] type:", type(out[0]))
    print("  item[0] repr:", repr(out[0])[:300])
    src_text = getattr(out[0], "text", out[0])
    print("  SOURCE TEXT:", src_text)
except Exception:
    traceback.print_exc()

# ---------------------------------------------------------------- MT: NLLB-200
print("\n=== MT: NLLB-200-distilled-600M (spa_Latn -> eng_Latn) ===")
translated = None
try:
    t0 = time.time()
    from transformers import AutoTokenizer, AutoModelForSeq2SeqLM
    name = "facebook/nllb-200-distilled-600M"
    tok = AutoTokenizer.from_pretrained(name)
    model = AutoModelForSeq2SeqLM.from_pretrained(name)
    print(f"  model loaded in {time.time()-t0:.1f}s")
    tok.src_lang = "spa_Latn"
    text_in = src_text or "Hola, me llamo Nicolas y estudio ingenieria de sistemas."
    inputs = tok(text_in, return_tensors="pt")
    bos = tok.convert_tokens_to_ids("eng_Latn")
    t1 = time.time()
    gen = model.generate(**inputs, forced_bos_token_id=bos, max_length=512)
    translated = tok.batch_decode(gen, skip_special_tokens=True)[0]
    print(f"  generate() in {time.time()-t1:.1f}s")
    print("  TRANSLATED:", translated)
except Exception:
    traceback.print_exc()

# ---------------------------------------------------------------- TTS: Piper
print("\n=== TTS: Piper ===")
text_for_tts = translated or "Hello, my name is Nicolas and I study systems engineering."
try:
    import wave
    from piper import PiperVoice
    t0 = time.time()
    voice = PiperVoice.load(f"{PIPER_VOICE}.onnx")
    print(f"  voice loaded in {time.time()-t0:.1f}s")
    with wave.open("spike_out.wav", "wb") as wf:
        voice.synthesize(text_for_tts, wf)
    print("  wrote spike_out.wav")
    print("  (PiperVoice.synthesize(text, wave_file) worked)")
except Exception:
    traceback.print_exc()
    print("  Piper Python API failed. Note the error. Fallbacks to try:")
    print("   - download voice: python -m piper.download_voices en_US-lessac-medium")
    print("   - or CLI: echo TEXT | piper -m en_US-lessac-medium.onnx -f spike_out.wav")

print("\nDone. Record the working APIs, return shapes, timings and RAM in python/SPIKE_NOTES.md")
