# webscrack

`webscrack` is a native Python package implemented in Rust with [PyO3](https://pyo3.rs/) and [Oxc](https://oxc.rs/). It provides fast JavaScript and TypeScript parsing, readable code generation, and minification without a Node.js runtime.

This initial port exposes the Oxc compiler primitives. It is intentionally not described as a complete one-to-one implementation of the upstream `webcrack` reverse-engineering pipeline: webpack/browserify unpacking, obfuscator.io-specific cleanup, and runtime-assisted transformations are separate features that can be added on top of the stable parser/codegen core.

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
```

`transform()` returns a `Result` with `code`, `bundle`, and `diagnostics` attributes. `Result.save(directory)` writes the generated code to `directory/index.js`.

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
