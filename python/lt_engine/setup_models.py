"""Pre-download all engine models. Run as `python -m lt_engine.setup_models`.

Extracted from an inline script embedded in the Rust setup so the
orchestration (what downloads, with what parallelism, what may fail) is
testable with pytest. Downloads are cached under HF_HOME; re-running is a
fast no-op when everything is already present.

Progress protocol: a background thread samples the cache directory size and
prints `PROGRESS:<pct>` lines on stdout, which the Rust setup maps onto its
overall progress bar. Works the same whether downloads run serially or in
parallel — per-download progress hooks cannot be aggregated across threads.
"""
from __future__ import annotations

import os
import sys
import threading
from concurrent.futures import ThreadPoolExecutor

# Sum of all model downloads, used to turn cache-dir growth into a rough
# overall percentage. Precision does not matter: the bar just has to move.
TOTAL_DOWNLOAD_BYTES = 5_600_000_000

PROGRESS_PREFIX = "PROGRESS:"


def _patch_windows_symlinks() -> None:
    """Make huggingface_hub survive Windows without Developer Mode.

    os.symlink raises WinError 1314 there, and the hub's fallback copies
    files using the relative symlink src resolved from CWD instead of from
    the symlink's parent — patch it so the copy always uses the correct
    absolute path.
    """
    if os.name != "nt":
        return
    try:
        import shutil

        import huggingface_hub.file_download as hfd

        def _safe_symlink(src, dst, new_blob=False):
            try:
                os.symlink(src, dst)
            except OSError:
                abs_src = src if os.path.isabs(src) else os.path.normpath(
                    os.path.join(os.path.dirname(dst), src)
                )
                if not os.path.exists(dst):
                    if new_blob:
                        shutil.move(abs_src, dst)
                    else:
                        shutil.copy2(abs_src, dst)

        hfd._create_symlink = _safe_symlink
    except Exception:  # noqa: BLE001 — rely on the huggingface_hub>=0.25 pin
        pass


def download_marian() -> None:
    from transformers import MarianMTModel, MarianTokenizer

    MarianTokenizer.from_pretrained("Helsinki-NLP/opus-mt-es-en")
    MarianMTModel.from_pretrained("Helsinki-NLP/opus-mt-es-en")


def download_parakeet() -> None:
    from huggingface_hub import snapshot_download

    snapshot_download("nvidia/parakeet-tdt-0.6b-v3")


def download_canary() -> None:
    from huggingface_hub import snapshot_download

    snapshot_download("nvidia/canary-1b-flash")


def download_pocket_tts() -> None:
    from pocket_tts import TTSModel

    TTSModel.load_model()


def download_all(max_workers: int = 2) -> None:
    """Download every model, largest first, `max_workers` at a time.

    max_workers is capped low on purpose: two streams overlap enough to help
    on connections a single stream cannot saturate, without hammering the
    Hub or thrashing slow disks. A failed required model raises after all
    downloads settle; Pocket TTS (voice cloning) is optional and only warns
    — the app falls back to the standard Piper voice.
    """
    _patch_windows_symlinks()

    required = [
        ("Canary 1B Flash (~3.5 GB)", download_canary),
        ("Parakeet ASR (~1.1 GB)", download_parakeet),
        ("MarianMT ES->EN (~300 MB)", download_marian),
    ]
    optional = [("Pocket TTS (~200 MB)", download_pocket_tts)]

    failed = []
    with ThreadPoolExecutor(max_workers=max_workers) as pool:
        required_futures = [(name, pool.submit(fn)) for name, fn in required]
        optional_futures = [(name, pool.submit(fn)) for name, fn in optional]

        for name, future in required_futures:
            try:
                future.result()
                print(f"{name} ready.", flush=True)
            except Exception as e:  # noqa: BLE001 — reported, then aggregated
                print(
                    f"ERROR [{name}]: {type(e).__name__}: {e}",
                    file=sys.stderr,
                    flush=True,
                )
                failed.append(name)

        for name, future in optional_futures:
            try:
                future.result()
                print(f"{name} ready.", flush=True)
            except Exception as e:  # noqa: BLE001 — optional, degrade only
                print(f"WARN [{name} optional]: {type(e).__name__}: {e}", flush=True)

    if failed:
        raise RuntimeError(f"model downloads failed: {', '.join(failed)}")


def progress_pct(bytes_done: int, total: int = TOTAL_DOWNLOAD_BYTES) -> int:
    """Overall percentage from cache size, clamped to 99 until completion."""
    if total <= 0 or bytes_done <= 0:
        return 0
    return min(99, int(bytes_done * 100 // total))


def _dir_size_bytes(path: str) -> int:
    total = 0
    for root, _dirs, files in os.walk(path):
        for name in files:
            try:
                total += os.path.getsize(os.path.join(root, name))
            except OSError:
                continue
    return total


def _report_progress_until(
    stop: threading.Event, cache_dir: str, interval_s: float = 2.0
) -> None:
    while not stop.wait(interval_s):
        print(f"{PROGRESS_PREFIX}{progress_pct(_dir_size_bytes(cache_dir))}", flush=True)


def main() -> int:
    cache_dir = os.environ.get("HF_HOME", "")
    stop = threading.Event()
    if cache_dir and os.path.isdir(cache_dir):
        threading.Thread(
            target=_report_progress_until, args=(stop, cache_dir), daemon=True
        ).start()
    try:
        download_all()
    except Exception as e:  # noqa: BLE001 — exit code is the contract with Rust
        print(f"ERROR: {type(e).__name__}: {e}", file=sys.stderr, flush=True)
        return 1
    finally:
        stop.set()
    print(f"{PROGRESS_PREFIX}100", flush=True)
    return 0


if __name__ == "__main__":
    sys.exit(main())
