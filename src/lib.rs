use oxc_allocator::Allocator;
use oxc_codegen::{Codegen, CodegenOptions};
use oxc_minifier::{Minifier, MinifierOptions};
use oxc_parser::{Parser, ParseOptions};
use oxc_span::SourceType;
use pyo3::exceptions::{PyIOError, PySyntaxError, PyValueError};
use pyo3::prelude::*;
use std::fs;
use std::path::Path;

fn source_type_for(filename: Option<&str>, source_type: &str) -> PyResult<SourceType> {
    if source_type == "js" || source_type == "javascript" {
        return Ok(SourceType::default());
    }
    if source_type == "jsx" {
        return Ok(SourceType::jsx());
    }
    if source_type == "ts" || source_type == "typescript" {
        return Ok(SourceType::ts());
    }
    if source_type == "tsx" {
        return Ok(SourceType::tsx());
    }
    if source_type == "mjs" {
        return Ok(SourceType::mjs());
    }
    if source_type == "cjs" {
        return Ok(SourceType::cjs());
    }
    if source_type == "auto" {
        if let Some(name) = filename {
            return SourceType::from_path(name).map_err(|e| {
                PyValueError::new_err(format!("cannot infer source type from {name}: {e}"))
            });
        }
        return Ok(SourceType::default());
    }
    Err(PyValueError::new_err(format!(
        "unsupported source_type {source_type:?}; use js, jsx, ts, tsx, mjs, cjs, or auto"
    )))
}

fn process(source: &str, filename: Option<&str>, source_type: &str, minify: bool) -> PyResult<(String, Vec<String>)> {
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
    let output = Codegen::new().with_options(CodegenOptions { minify, ..CodegenOptions::default() }).build(&program);
    Ok((output.code, diagnostics))
}

#[pyclass]
#[derive(Clone)]
pub struct Result {
    #[pyo3(get)]
    pub code: String,
    #[pyo3(get)]
    pub bundle: bool,
    #[pyo3(get)]
    pub diagnostics: Vec<String>,
}

#[pymethods]
impl Result {
    #[pyo3(signature = (directory))]
    pub fn save(&self, directory: &str) -> PyResult<()> {
        fs::create_dir_all(directory).map_err(|e| PyIOError::new_err(e.to_string()))?;
        let path = Path::new(directory).join("index.js");
        fs::write(path, &self.code).map_err(|e| PyIOError::new_err(e.to_string()))
    }
    fn __repr__(&self) -> String {
        format!("Result(code_len={}, bundle={})", self.code.len(), self.bundle)
    }
}

#[pyfunction]
#[pyo3(signature = (source, *, filename=None, source_type="auto", minify=false))]
pub fn transform(source: &str, filename: Option<&str>, source_type: &str, minify: bool) -> PyResult<Result> {
    let (code, diagnostics) = process(source, filename, source_type, minify)?;
    Ok(Result { code, bundle: false, diagnostics })
}

#[pyfunction]
#[pyo3(signature = (source, *, filename=None, source_type="auto"))]
pub fn format(source: &str, filename: Option<&str>, source_type: &str) -> PyResult<String> {
    Ok(process(source, filename, source_type, false)?.0)
}

#[pyfunction]
#[pyo3(signature = (source, *, filename=None, source_type="auto"))]
pub fn minify(source: &str, filename: Option<&str>, source_type: &str) -> PyResult<String> {
    Ok(process(source, filename, source_type, true)?.0)
}

#[pymodule]
fn _native(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<Result>()?;
    m.add_function(wrap_pyfunction!(transform, m)?)?;
    m.add_function(wrap_pyfunction!(format, m)?)?;
    m.add_function(wrap_pyfunction!(minify, m)?)?;
    Ok(())
}
