# webscrack

`webscrack` is a native Python package implemented in Rust with [PyO3](https://pyo3.rs/) and [Oxc](https://oxc.rs/). It provides fast JavaScript and TypeScript parsing, readable code generation, minification, bookmarklet normalization, safe unminification, and a compatibility-oriented `webcrack()` pipeline without a Node.js runtime. Deobfuscation includes a non-executing static subset for literal string arrays and simple decoder functions; it never runs JavaScript in the host.

The Rust implementation follows the upstream option surface (`jsx`, `unpack`, `deobfuscate`, `unminify`, and `mangle`) and returns `Result`/`Bundle`/`Module` objects with `save()` methods. Oxc provides the safe parser, generator, and minifier core. Runtime-assisted obfuscator decoding and full webpack/browserify module extraction remain separate follow-up work; bundle detection is included and safely materializes the input as an entry module.

The Rust source is organized into the corresponding implementation areas: `src/ast_utils.rs` contains shared AST/path/bookmarklet helpers, `src/deobfuscate.rs` contains the non-executing static decoder and debug-cleanup helpers, and `src/unpack.rs` contains webpack/Browserify detection and module-path helpers. These modules are wired into the native API and covered by Rust and Python regression tests. The static deobfuscator recognizes literal arrays (including `void 0` entries), numeric literal index arithmetic, wrapped array accessors, and direct decoder calls. It intentionally leaves dynamic expressions, mutable or aliased arrays, array rotators, RC4/Base64/custom decoders, conditional calls, object-property argument lifting, function aliases, and control-flow transforms unchanged; it does not claim to execute arbitrary upstream `isolated-vm` decoder code.

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
