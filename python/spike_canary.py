"""Spike: measure Canary 1B Flash (AST es->en) vs current Parakeet+Marian on CPU.

Decision gate for replacing the two-stage ASR+MT pipeline with a single
speech-translation model. Run manually:

    python spike_canary.py [path/to/spanish.wav]

Without an argument it synthesizes an English test clip with Piper (enough
for timing: compute cost does not depend on input language, only duration).
Quality of es->en translation must be judged with real Spanish audio.
"""
import sys
import time
import wave
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
TEST_WAV = REPO_ROOT / "spike-canary-input.wav"

TEST_SENTENCE = (
    "The quick brown fox jumps over the lazy dog near the riverbank, "
    "and the afternoon sun keeps shining over the quiet valley while "
    "people walk slowly through the old market streets of the city."
)


def make_test_wav() -> Path:
    if TEST_WAV.exists():
        return TEST_WAV
    from piper import PiperVoice

    voice = PiperVoice.load(str(REPO_ROOT / "en_US-lessac-medium.onnx"))
    with wave.open(str(TEST_WAV), "wb") as wf:
        voice.synthesize_wav(TEST_SENTENCE, wf)
    return TEST_WAV


def wav_duration_s(path: Path) -> float:
    with wave.open(str(path), "rb") as wf:
        return wf.getnframes() / wf.getframerate()


def bench(label: str, fn, runs: int = 3):
    # First call includes model load / warmup; report it separately.
    t0 = time.perf_counter()
    out = fn()
    first = time.perf_counter() - t0
    times = []
    for _ in range(runs):
        t0 = time.perf_counter()
        out = fn()
        times.append(time.perf_counter() - t0)
    best = min(times)
    print(f"[{label}] first(load+run): {first:.2f}s | best of {runs}: {best:.2f}s")
    print(f"[{label}] output: {out!r:.300}")
    return best


def main() -> None:
    audio = Path(sys.argv[1]) if len(sys.argv) > 1 else make_test_wav()
    dur = wav_duration_s(audio)
    print(f"input: {audio} ({dur:.1f}s)")

    # -- Baseline: current pipeline (Parakeet ASR + Marian MT) ---------------
    import lt_engine.pipeline as pipeline

    def baseline():
        text = pipeline.transcribe(str(audio))
        return pipeline.translate(text)

    base_s = bench("parakeet+marian", baseline)

    # -- Candidate: Canary 1B Flash, AST in one pass -------------------------
    from nemo.collections.asr.models import EncDecMultiTaskModel

    canary = EncDecMultiTaskModel.from_pretrained("nvidia/canary-1b-flash")
    canary.eval()
    decode_cfg = canary.cfg.decoding
    decode_cfg.beam.beam_size = 1  # greedy: latency over last-drop of quality
    canary.change_decoding_strategy(decode_cfg)

    def canary_ast():
        out = canary.transcribe(
            [str(audio)],
            source_lang="es",
            target_lang="en",
            task="ast",
            pnc="yes",
            batch_size=1,
            verbose=False,
        )
        item = out[0]
        return getattr(item, "text", item)

    canary_s = bench("canary-1b-flash AST", canary_ast)

    print()
    print(f"audio duration:        {dur:.1f}s")
    print(f"baseline  RTF: {base_s / dur:.2f}  ({base_s:.2f}s)")
    print(f"canary    RTF: {canary_s / dur:.2f}  ({canary_s:.2f}s)")
    print("RTF < ~0.5 keeps the live pipeline comfortable on weaker CPUs.")


if __name__ == "__main__":
    main()
