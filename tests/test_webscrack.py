from pathlib import Path

import webscrack


def test_format_and_result():
    result = webscrack.transform("const x=1+2; console.log(x)")
    assert "const" in result.code
    assert result.bundle is False
    assert result.diagnostics == []


def test_minify():
    output = webscrack.minify("const value = 1 + 2; console.log(value);")
    assert "console.log" in output
    assert len(output) < len("const value = 1 + 2; console.log(value);")


def test_typescript():
    output = webscrack.format("const value: number = 42", source_type="ts")
    assert "number" in output


def test_save(tmp_path: Path):
    result = webscrack.transform("export const x = 1", source_type="mjs")
    result.save(str(tmp_path))
    assert (tmp_path / "index.js").read_text()
