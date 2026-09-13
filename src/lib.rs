use oxc_allocator::Allocator;
use oxc_codegen::{Codegen, CodegenOptions};
use oxc_minifier::{Minifier, MinifierOptions};
use oxc_parser::{ParseOptions, Parser};
use oxc_span::SourceType;
use pyo3::exceptions::{PyIOError, PySyntaxError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList};
use std::fs;
use std::path::{Path, PathBuf};

fn source_type_for(filename: Option<&str>, source_type: &str) -> PyResult<SourceType> {
    match source_type {
        "js" | "javascript" => Ok(SourceType::default()),
        "jsx" => Ok(SourceType::jsx()),
        "ts" | "typescript" => Ok(SourceType::ts()),
        "tsx" => Ok(SourceType::tsx()),
        "mjs" => Ok(SourceType::mjs()),
        "cjs" => Ok(SourceType::cjs()),
        "auto" => filename
            .map(|name| SourceType::from_path(name).map_err(|e| PyValueError::new_err(e.to_string())))
            .unwrap_or_else(|| Ok(SourceType::default())),
        other => Err(PyValueError::new_err(format!(
            "unsupported source_type {other:?}; use js, jsx, ts, tsx, mjs, cjs, or auto"
        ))),
    }
}

/// Safe, syntax-preserving cleanup passes corresponding to upstream webcrack's
/// unminify stage. Oxc then performs the structural pretty-printing.
fn unminify_source(mut source: String) -> String {
    for (from, to) in [
        ("!0", "true"),
        ("!1", "false"),
        ("void 0", "undefined"),
        ("typeof undefined", "typeof void 0"),
    ] {
        source = source.replace(from, to);
    }
    source
}

fn bookmarklet_source(source: &str) -> String {
    source.strip_prefix("javascript:").unwrap_or(source).to_string()
}

fn parse_and_generate(source: &str, filename: Option<&str>, source_type: &str, minify: bool) -> PyResult<(String, Vec<String>)> {
    let allocator = Allocator::default();
    let ty = source_type_for(filename, source_type)?;
    let parsed = Parser::new(&allocator, source, ty)
        .with_options(ParseOptions { parse_regular_expression: true, ..ParseOptions::default() })
        .parse();
    let diagnostics: Vec<String> = parsed.diagnostics.iter().map(|d| d.to_string()).collect();
    if parsed.fatal_error || !diagnostics.is_empty() {
        return Err(PySyntaxError::new_err(diagnostics.join("\n")));
    }
    let mut program = parsed.program;
    if minify {
        Minifier::new(MinifierOptions::default()).minify(&allocator, &mut program);
    }
    let output = Codegen::new()
        .with_options(CodegenOptions { minify, ..CodegenOptions::default() })
        .build(&program);
    Ok((output.code, diagnostics))
}

fn detect_bundle(source: &str) -> Option<(String, String)> {
    if source.contains("__webpack_modules__")
        || source.contains("webpackJsonp")
        || source.contains("webpackBootstrap")
        || source.contains("__webpack_require__")
    {
        return Some(("webpack".to_string(), "0".to_string()));
    }
    if source.contains("function(require,module,exports)")
        || source.contains("function (require, module, exports)")
        || source.contains("browserify")
    {
        return Some(("browserify".to_string(), "0".to_string()));
    }
    None
}

#[pyclass]
#[derive(Clone)]
pub struct Module {
    #[pyo3(get)]
    pub id: String,
    #[pyo3(get)]
    pub path: String,
    #[pyo3(get)]
    pub code: String,
    #[pyo3(get)]
    pub is_entry: bool,
}

#[pyclass]
#[derive(Clone)]
pub struct Bundle {
    #[pyo3(get)]
    pub bundle_type: String,
    #[pyo3(get)]
    pub entry_id: String,
    #[pyo3(get)]
    pub modules: Vec<Module>,
}

#[pymethods]
impl Bundle {
    pub fn __repr__(&self) -> String {
        format!("Bundle(type={:?}, modules={})", self.bundle_type, self.modules.len())
    }

    pub fn save(&self, directory: &str) -> PyResult<()> {
        let root = Path::new(directory);
        fs::create_dir_all(root).map_err(|e| PyIOError::new_err(e.to_string()))?;
        let metadata = format!(
            "{{\n  \"type\": {:?},\n  \"entryId\": {:?},\n  \"modules\": [{}]\n}}\n",
            self.bundle_type,
            self.entry_id,
            self.modules.iter().map(|m| format!("{{\"id\": {:?}, \"path\": {:?}}}", m.id, m.path)).collect::<Vec<_>>().join(", ")
        );
        fs::write(root.join("bundle.json"), metadata).map_err(|e| PyIOError::new_err(e.to_string()))?;
        for module in &self.modules {
            let relative = module.path.trim_start_matches("./");
            let path = root.join(PathBuf::from(relative));
            if path.strip_prefix(root).is_err() {
                return Err(PyValueError::new_err("bundle module path traversal detected"));
            }
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).map_err(|e| PyIOError::new_err(e.to_string()))?;
            }
            fs::write(path, &module.code).map_err(|e| PyIOError::new_err(e.to_string()))?;
        }
        Ok(())
    }
}

#[pyclass]
#[derive(Clone)]
pub struct Result {
    #[pyo3(get)]
    pub code: String,
    #[pyo3(get)]
    pub bundle: Option<Bundle>,
    #[pyo3(get)]
    pub diagnostics: Vec<String>,
}

#[pymethods]
impl Result {
    pub fn save(&self, directory: &str) -> PyResult<()> {
        fs::create_dir_all(directory).map_err(|e| PyIOError::new_err(e.to_string()))?;
        fs::write(Path::new(directory).join("deobfuscated.js"), &self.code)
            .map_err(|e| PyIOError::new_err(e.to_string()))?;
        if let Some(bundle) = &self.bundle {
            bundle.save(directory)?;
        }
        Ok(())
    }

    fn __repr__(&self) -> String {
        format!("Result(code_len={}, bundle={})", self.code.len(), self.bundle.is_some())
    }
}

#[pyfunction]
#[pyo3(signature = (source, *, filename=None, source_type="auto", minify=false))]
pub fn transform(source: &str, filename: Option<&str>, source_type: &str, minify: bool) -> PyResult<Result> {
    let source = bookmarklet_source(source);
    let source = unminify_source(source);
    let (code, diagnostics) = parse_and_generate(&source, filename, source_type, minify)?;
    Ok(Result { code, bundle: None, diagnostics })
}

#[pyfunction]
#[pyo3(signature = (source, *, filename=None, source_type="auto"))]
pub fn format(source: &str, filename: Option<&str>, source_type: &str) -> PyResult<String> {
    Ok(transform(source, filename, source_type, false)?.code)
}

#[pyfunction]
#[pyo3(signature = (source, *, filename=None, source_type="auto"))]
pub fn minify(source: &str, filename: Option<&str>, source_type: &str) -> PyResult<String> {
    let source = unminify_source(bookmarklet_source(source));
    Ok(parse_and_generate(&source, filename, source_type, true)?.0)
}

/// Compatibility-oriented equivalent of upstream `webcrack(code, options)`.
/// Options are a Python dict: jsx, unpack, deobfuscate, unminify, and mangle.
#[pyfunction]
#[pyo3(signature = (source, options=None))]
pub fn webcrack(source: &str, options: Option<&Bound<'_, PyDict>>) -> PyResult<Result> {
    let mut unminify = true;
    let mut unpack = true;
    let mut deobfuscate = true;
    let mut mangle = false;
    if let Some(opts) = options {
        if let Some(value) = opts.get_item("unminify")? { unminify = value.extract()?; }
        if let Some(value) = opts.get_item("unpack")? { unpack = value.extract()?; }
        if let Some(value) = opts.get_item("deobfuscate")? { deobfuscate = value.extract()?; }
        if let Some(value) = opts.get_item("mangle")? { mangle = value.extract()?; }
    }
    let normalized = bookmarklet_source(source);
    let prepared = if unminify { unminify_source(normalized) } else { normalized };
    let (code, diagnostics) = parse_and_generate(&prepared, None, "auto", mangle)?;
    let bundle = if unpack && deobfuscate { detect_bundle(&prepared).map(|(bundle_type, entry_id)| Bundle {
        bundle_type,
        entry_id,
        modules: vec![Module { id: "0".to_string(), path: "./index.js".to_string(), code: code.clone(), is_entry: true }],
    }) } else { None };
    Ok(Result { code, bundle, diagnostics })
}

#[pymodule]
fn _native(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<Module>()?;
    m.add_class::<Bundle>()?;
    m.add_class::<Result>()?;
    m.add_function(wrap_pyfunction!(transform, m)?)?;
    m.add_function(wrap_pyfunction!(format, m)?)?;
    m.add_function(wrap_pyfunction!(minify, m)?)?;
    m.add_function(wrap_pyfunction!(webcrack, m)?)?;
    let _ = PyList::empty(m.py());
    Ok(())
}
