import argparse
import json
import os
import sys

from .pipeline import translate_audio


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

    try:
        result = translate_audio(args.file, args.out_dir, args.src, args.tgt)
    except ValueError as e:
        print(str(e), file=sys.stderr)
        return 1

    with open(os.path.join(args.out_dir, "result.json"), "w", encoding="utf-8") as f:
        json.dump(
            {
                "source_text": result["source_text"],
                "translated_text": result["translated_text"],
            },
            f,
            ensure_ascii=False,
        )

    print("done")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
