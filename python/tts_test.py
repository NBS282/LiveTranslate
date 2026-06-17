"""Quick Piper TTS check (no Parakeet load). Confirms synthesize_wav writes a valid wav."""
import wave
from piper import PiperVoice

voice = PiperVoice.load("en_US-lessac-medium.onnx")
with wave.open("tts_test.wav", "wb") as wf:
    voice.synthesize_wav("Hello, this is a test of the translated voice.", wf)
print("OK: wrote tts_test.wav")
