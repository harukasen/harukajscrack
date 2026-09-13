import argparse
import pathlib
import sys
from . import transform


def main() -> int:
    parser = argparse.ArgumentParser(prog="webscrack", description="Format or minify JavaScript/TypeScript with Oxc")
    parser.add_argument("input", nargs="?", help="input file; omit to read stdin")
    parser.add_argument("-o", "--output", help="output file")
    parser.add_argument("--minify", action="store_true", help="compress and minify output")
    parser.add_argument("--source-type", default="auto", choices=["auto", "js", "jsx", "ts", "tsx", "mjs", "cjs"])
    args = parser.parse_args()
    filename = args.input
    source = pathlib.Path(filename).read_text() if filename else sys.stdin.read()
    result = transform(source, filename=filename, source_type=args.source_type, minify=args.minify)
    if args.output:
        pathlib.Path(args.output).write_text(result.code)
    else:
        sys.stdout.write(result.code)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
