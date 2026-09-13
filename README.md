# webscrack

`webscrack` is a native Python package implemented in Rust with [PyO3](https://pyo3.rs/) and [Oxc](https://oxc.rs/). It provides fast JavaScript and TypeScript parsing, readable code generation, minification, bookmarklet normalization, safe unminification, and a compatibility-oriented `webcrack()` pipeline without a Node.js runtime.

The Rust implementation follows the upstream option surface (`jsx`, `unpack`, `deobfuscate`, `unminify`, and `mangle`) and returns `Result`/`Bundle`/`Module` objects with `save()` methods. Oxc provides the safe parser, generator, and minifier core. Runtime-assisted obfuscator decoding and full webpack/browserify module extraction remain separate follow-up work; bundle detection is included and safely materializes the input as an entry module.

## Install

From a checkout:

```bash
python -m pip install .
```

After publishing a wheel:

```bash
python -m pip install webscrack
```

The build requires Rust, a C compiler, and Python 3.9 or newer. Wheels built with maturin contain the native extension.

## Python API

```python
import webscrack

source = "const answer=1+1; console.log(answer)"
result = webscrack.transform(source)
print(result.code)

compressed = webscrack.minify(source)
formatted_typescript = webscrack.format("const value: number = 42", source_type="ts")

result = webscrack.webcrack(source, {
    "unminify": True,
    "deobfuscate": True,
    "unpack": True,
    "mangle": False,
})
```

`transform()` and `webcrack()` return a `Result` with `code`, `bundle`, and `diagnostics` attributes. `Result.save(directory)` writes `deobfuscated.js`, `bundle.json`, and any extracted modules. `Bundle.save(directory)` is also available directly.

Supported source types are `auto`, `js`, `jsx`, `ts`, `tsx`, `mjs`, and `cjs`. With `auto`, the type is inferred from `filename` when one is supplied.

## CLI

```bash
webscrack input.js
webscrack input.js --minify -o output.js
cat input.ts | webscrack --source-type ts
python -m webscrack input.js
```

## Development

```bash
python -m pip install maturin
maturin develop --release
python -c 'import webscrack; print(webscrack.format("const x=1"))'
cargo test
```

## License

MIT
