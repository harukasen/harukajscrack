from pathlib import Path

import webscrack


def test_format_and_result():
    result = webscrack.transform("const x=1+2; console.log(x)")
    assert "const" in result.code
    assert result.bundle is None
    assert result.diagnostics == []


def test_minify():
    output = webscrack.minify("const value = 1 + 2; console.log(value);")
    assert "console.log" in output
    assert len(output) < len("const value = 1 + 2; console.log(value);")


def test_typescript():
    output = webscrack.format("const value: number = 42", source_type="ts")
    assert "number" in output


def test_webcrack_compatibility_options():
    result = webscrack.webcrack("javascript:const flag=!0;", {"unpack": False})
    assert "true" in result.code
    assert result.bundle is None


def test_public_pipeline_entry_points():
    assert "true" in webscrack.unminify("const flag=!0;")
    assert "true" in webscrack.deobfuscate("const flag=!0;")
    assert webscrack.unpack("const x = 1;") is None


def test_static_string_array_inlining_matches_upstream_shape():
    source = "const arr = ['log', 'Hello, World!']; console[arr[0]](arr[1]);"
    output = webscrack.deobfuscate(source)
    assert 'console["log"]("Hello, World!")' in output
    assert "const arr" not in output


def test_static_wrapped_decoder_and_sparse_value():
    source = (
        "function a(){var x=['log',void 0];"
        "a=function(){return x;};return a();}"
        "function d(n){var x=a();n=n-1;return x[n];}"
        "console.log(d(1));console.log(d(2));"
    )
    output = webscrack.deobfuscate(source)
    assert 'console.log("log")' in output
    assert "console.log(undefined)" in output
    assert "function d" not in output


def test_mutated_string_array_is_left_unchanged():
    source = "const arr = ['log', 'Hello']; arr[0] = 'warn'; console[arr[0]](arr[1]);"
    output = webscrack.deobfuscate(source)
    assert 'arr[0] = "warn"' in output
    assert "console[arr[0]](arr[1])" in output


def test_bundle_detection_and_save(tmp_path: Path):
    result = webscrack.webcrack("var __webpack_modules__ = {};", {})
    assert result.bundle is not None
    result.save(str(tmp_path))
    assert (tmp_path / "deobfuscated.js").read_text()
    assert (tmp_path / "bundle.json").read_text()
