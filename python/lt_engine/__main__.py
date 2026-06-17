import argparse
import json
import os
import sys

from .pipeline import normalize_lang, transcribe, translate, synthesize


def main() -> int:
    ap = argparse.ArgumentParser(
        description="Offline Spanish-to-English audio translation pipeline."
    )
    ap.add_argument("--file", required=True, help="Path to input WAV file")
    ap.add_argument("--out-dir", required=True, help="Directory for output files")
    ap.add_argument("--src", default="es", help="Source language (default: es)")
    ap.add_argument("--tgt", default="en", help="Target language (default: en)")
    args = ap.parse_args()

    if not os.path.isfile(args.file):
        print(f"input not found: {args.file}", file=sys.stderr)
        return 2

    os.makedirs(args.out_dir, exist_ok=True)

    src = normalize_lang(args.src)
    tgt = normalize_lang(args.tgt)

    print(f"[1/3] Transcribing {args.file} ...", flush=True)
    source_text = transcribe(args.file)
    print(f"      source: {source_text}", flush=True)

    if not source_text.strip():
        print("transcription produced no text", file=sys.stderr)
        return 1

    print(f"[2/3] Translating {src} -> {tgt} ...", flush=True)
    translated_text = translate(source_text, src, tgt)
    print(f"      translation: {translated_text}", flush=True)

    out_wav = os.path.join(args.out_dir, "output.wav")
    print(f"[3/3] Synthesizing speech -> {out_wav} ...", flush=True)
    synthesize(translated_text, out_wav)

    result_json = os.path.join(args.out_dir, "result.json")
    with open(result_json, "w", encoding="utf-8") as f:
        json.dump(
            {"source_text": source_text, "translated_text": translated_text},
            f,
            ensure_ascii=False,
            indent=2,
        )

    print("done", flush=True)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
