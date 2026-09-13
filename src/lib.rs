use oxc_allocator::{Allocator, ArenaVec, GetAllocator, ReplaceWith};
use oxc_ast::{ast::*, builder::AstBuilder};
use oxc_ast_visit::VisitJsMut;
use oxc_codegen::{Codegen, CodegenOptions};
use oxc_minifier::{Minifier, MinifierOptions};
use oxc_parser::{ParseOptions, Parser};
use oxc_semantic::SemanticBuilder;
use oxc_span::{GetSpan, SourceType};
use pyo3::exceptions::{PyIOError, PySyntaxError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList};
use serde_json::Value as JsonValue;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

mod ast_utils;
#[path = "deobfuscate.rs"]
mod deobfuscate_utils;
#[path = "unpack.rs"]
mod unpack_utils;

fn source_type_for(filename: Option<&str>, source_type: &str) -> PyResult<SourceType> {
    match source_type {
        "js" | "javascript" => Ok(SourceType::default()),
        "jsx" => Ok(SourceType::jsx()),
        "ts" | "typescript" => Ok(SourceType::ts()),
        "tsx" => Ok(SourceType::tsx()),
        "mjs" => Ok(SourceType::mjs()),
        "cjs" => Ok(SourceType::cjs()),
        "auto" => filename
            .map(|name| {
                SourceType::from_path(name).map_err(|e| PyValueError::new_err(e.to_string()))
            })
            .unwrap_or_else(|| Ok(SourceType::default())),
        other => Err(PyValueError::new_err(format!(
            "unsupported source_type {other:?}; use js, jsx, ts, tsx, mjs, cjs, or auto"
        ))),
    }
}

/// AST-only safe subset of webcrack's unminify stage.
///
/// In particular, this deliberately does not operate on source text: strings,
/// comments, regular expressions, and formatting are therefore never rewritten.
/// The pass is conservative around bindings whose global meaning can be changed
/// by a local declaration (undefined, Infinity, and JSON).
struct LiteralUnminifier<'a> {
    builder: AstBuilder<'a>,
    shadowed: HashSet<String>,
}

impl<'a> LiteralUnminifier<'a> {
    fn new(allocator: &'a Allocator, shadowed: HashSet<String>) -> Self {
        Self {
            builder: AstBuilder::new(allocator),
            shadowed,
        }
    }

    fn identifier(&self, span: oxc_span::Span, name: &str) -> Expression<'a> {
        Expression::new_identifier(
            span,
            self.builder.allocator().alloc_str(name),
            &self.builder,
        )
    }

    fn number(&self, span: oxc_span::Span, value: f64) -> Expression<'a> {
        Expression::new_numeric_literal(
            span,
            value,
            None,
            oxc_syntax::number::NumberBase::Decimal,
            &self.builder,
        )
    }

    fn numeric(expr: &Expression<'_>) -> Option<f64> {
        match expr {
            Expression::NumericLiteral(lit) => Some(lit.value),
            _ => None,
        }
    }

    fn js_i32(value: f64) -> i32 {
        if !value.is_finite() || value == 0.0 {
            return 0;
        }
        let n = value.trunc() as i64;
        n as i32
    }

    fn fold_json(&self, value: &JsonValue, span: oxc_span::Span) -> Option<Expression<'a>> {
        match value {
            JsonValue::Null => Some(Expression::new_null_literal(span, &self.builder)),
            JsonValue::Bool(value) => {
                Some(Expression::new_boolean_literal(span, *value, &self.builder))
            }
            JsonValue::Number(value) => value
                .as_f64()
                .filter(|n| n.is_finite())
                .map(|n| self.number(span, n)),
            JsonValue::String(value) => Some(Expression::new_string_literal(
                span,
                self.builder.allocator().alloc_str(value),
                None,
                &self.builder,
            )),
            JsonValue::Array(values) => {
                let mut elements = ArenaVec::new_in(&self.builder);
                for value in values {
                    elements.push(self.fold_json(value, span)?.into());
                }
                Some(Expression::new_array_expression(
                    span,
                    elements,
                    &self.builder,
                ))
            }
            JsonValue::Object(values) => {
                let mut properties = ArenaVec::new_in(&self.builder);
                for (key, value) in values {
                    let value = self.fold_json(value, span)?;
                    let key = PropertyKey::from(Expression::new_string_literal(
                        span,
                        self.builder.allocator().alloc_str(key),
                        None,
                        &self.builder,
                    ));
                    properties.push(ObjectPropertyKind::new_object_property(
                        span,
                        PropertyKind::Init,
                        key,
                        value,
                        false,
                        false,
                        false,
                        &self.builder,
                    ));
                }
                Some(Expression::new_object_expression(
                    span,
                    properties,
                    &self.builder,
                ))
            }
        }
    }

    fn fold(&self, expr: Expression<'a>) -> Expression<'a> {
        let span = expr.span();
        match &expr {
            Expression::UnaryExpression(unary) => {
                let unary = unary.as_ref();
                match unary.operator {
                    oxc_syntax::operator::UnaryOperator::LogicalNot => {
                        if let Some(value) = Self::numeric(&unary.argument) {
                            if value == 0.0 || value == 1.0 {
                                return Expression::new_boolean_literal(
                                    span,
                                    value == 0.0,
                                    &self.builder,
                                );
                            }
                        }
                    }
                    oxc_syntax::operator::UnaryOperator::Void => {
                        if matches!(&unary.argument, Expression::NumericLiteral(lit) if lit.value == 0.0)
                            && !self.shadowed.contains("undefined")
                        {
                            return self.identifier(span, "undefined");
                        }
                    }
                    oxc_syntax::operator::UnaryOperator::UnaryPlus => {
                        if let Some(value) = Self::numeric(&unary.argument) {
                            if !(value == 0.0 && value.is_sign_negative()) {
                                return self.number(span, value);
                            }
                        }
                    }
                    oxc_syntax::operator::UnaryOperator::UnaryNegation => {
                        if let Some(value) = Self::numeric(&unary.argument) {
                            let value = -value;
                            if !(value == 0.0 && value.is_sign_negative()) {
                                return self.number(span, value);
                            }
                        }
                    }
                    oxc_syntax::operator::UnaryOperator::BitwiseNot => {
                        if let Some(value) = Self::numeric(&unary.argument) {
                            return self.number(span, f64::from(!Self::js_i32(value)));
                        }
                    }
                    _ => {}
                }
            }
            Expression::BinaryExpression(binary) => {
                let binary = binary.as_ref();
                let left = Self::numeric(&binary.left);
                let right = Self::numeric(&binary.right);
                if binary.operator == oxc_syntax::operator::BinaryOperator::Addition {
                    if let (Expression::StringLiteral(left), Expression::StringLiteral(right)) =
                        (&binary.left, &binary.right)
                    {
                        let mut value = left.value.to_string();
                        value.push_str(right.value.as_str());
                        return Expression::new_string_literal(
                            span,
                            self.builder.allocator().alloc_str(&value),
                            None,
                            &self.builder,
                        );
                    }
                }
                if let (Some(left), Some(right)) = (left, right) {
                    let value = match binary.operator {
                        oxc_syntax::operator::BinaryOperator::Addition => left + right,
                        oxc_syntax::operator::BinaryOperator::Subtraction => left - right,
                        oxc_syntax::operator::BinaryOperator::Multiplication => left * right,
                        oxc_syntax::operator::BinaryOperator::Division => {
                            if right == 0.0 && !self.shadowed.contains("Infinity") {
                                if left.is_sign_negative() {
                                    return Expression::new_unary_expression(
                                        span,
                                        oxc_syntax::operator::UnaryOperator::UnaryNegation,
                                        self.identifier(span, "Infinity"),
                                        &self.builder,
                                    );
                                }
                                return self.identifier(span, "Infinity");
                            }
                            left / right
                        }
                        oxc_syntax::operator::BinaryOperator::Remainder => left % right,
                        oxc_syntax::operator::BinaryOperator::Exponential => left.powf(right),
                        oxc_syntax::operator::BinaryOperator::BitwiseOR => {
                            f64::from(Self::js_i32(left) | Self::js_i32(right))
                        }
                        oxc_syntax::operator::BinaryOperator::BitwiseXOR => {
                            f64::from(Self::js_i32(left) ^ Self::js_i32(right))
                        }
                        oxc_syntax::operator::BinaryOperator::BitwiseAnd => {
                            f64::from(Self::js_i32(left) & Self::js_i32(right))
                        }
                        oxc_syntax::operator::BinaryOperator::ShiftLeft => {
                            f64::from(Self::js_i32(left) << (Self::js_i32(right) & 31))
                        }
                        oxc_syntax::operator::BinaryOperator::ShiftRight => {
                            f64::from(Self::js_i32(left) >> (Self::js_i32(right) & 31))
                        }
                        oxc_syntax::operator::BinaryOperator::ShiftRightZeroFill => {
                            f64::from((Self::js_i32(left) as u32) >> (Self::js_i32(right) & 31))
                        }
                        _ => return expr,
                    };
                    if !(value == 0.0 && value.is_sign_negative())
                        && (value.is_finite() || value.is_infinite())
                    {
                        return self.number(span, value);
                    }
                }
            }
            Expression::CallExpression(call) => {
                if self.shadowed.contains("JSON") || call.arguments.len() != 1 {
                    return expr;
                }
                if let Expression::StaticMemberExpression(member) = &call.callee {
                    if let Expression::Identifier(object) = &member.object {
                        if object.name == "JSON"
                            && member.property.name == "parse"
                            && !call.optional
                        {
                            if let Some(Argument::StringLiteral(string)) = call.arguments.first() {
                                if let Ok(value) =
                                    serde_json::from_str::<JsonValue>(string.value.as_str())
                                {
                                    if let Some(replacement) = self.fold_json(&value, span) {
                                        return replacement;
                                    }
                                }
                            }
                        }
                    }
                }
            }
            _ => {}
        }
        expr
    }
}

impl<'a> VisitJsMut<'a> for LiteralUnminifier<'a> {
    fn visit_binding_identifier(&mut self, it: &mut BindingIdentifier<'a>) {
        self.shadowed.insert(it.name.to_string());
    }

    fn visit_expression(&mut self, it: &mut Expression<'a>) {
        oxc_ast_visit::walk_js_mut::walk_expression(self, it);
        it.replace_with(|expr| self.fold(expr));
    }
}

fn unminify_program<'a>(allocator: &'a Allocator, program: &mut Program<'a>) {
    // Build Oxc's semantic model as the scope-analysis boundary. The visitor
    // additionally records every binding, conservatively covering nested scopes.
    let _ = SemanticBuilder::new_compiler().build(program);
    let mut pass = LiteralUnminifier::new(allocator, HashSet::new());
    pass.visit_program(program);
}

fn bookmarklet_source(source: &str) -> String {
    ast_utils::normalize_bookmarklet(source)
}

fn parse_and_generate(
    source: &str,
    filename: Option<&str>,
    source_type: &str,
    minify: bool,
    apply_unminify: bool,
) -> PyResult<(String, Vec<String>)> {
    let allocator = Allocator::default();
    let ty = source_type_for(filename, source_type)?;
    let parsed = Parser::new(&allocator, source, ty)
        .with_options(ParseOptions {
            parse_regular_expression: true,
            ..ParseOptions::default()
        })
        .parse();
    let diagnostics: Vec<String> = parsed.diagnostics.iter().map(|d| d.to_string()).collect();
    if parsed.fatal_error || !diagnostics.is_empty() {
        return Err(PySyntaxError::new_err(diagnostics.join("\n")));
    }
    let mut program = parsed.program;
    if apply_unminify {
        unminify_program(&allocator, &mut program);
    }
    if minify {
        Minifier::new(MinifierOptions::default()).minify(&allocator, &mut program);
    }
    let output = Codegen::new()
        .with_options(CodegenOptions {
            minify,
            ..CodegenOptions::default()
        })
        .build(&program);
    Ok((output.code, diagnostics))
}

fn detect_bundle(source: &str) -> Option<(String, String)> {
    unpack_utils::detect(source).map(|bundle| (bundle.kind, bundle.entry_id))
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
        format!(
            "Bundle(type={:?}, modules={})",
            self.bundle_type,
            self.modules.len()
        )
    }

    pub fn save(&self, directory: &str) -> PyResult<()> {
        let root = Path::new(directory);
        fs::create_dir_all(root).map_err(|e| PyIOError::new_err(e.to_string()))?;
        let metadata = format!(
            "{{\n  \"type\": {:?},\n  \"entryId\": {:?},\n  \"modules\": [{}]\n}}\n",
            self.bundle_type,
            self.entry_id,
            self.modules
                .iter()
                .map(|m| format!("{{\"id\": {:?}, \"path\": {:?}}}", m.id, m.path))
                .collect::<Vec<_>>()
                .join(", ")
        );
        fs::write(root.join("bundle.json"), metadata)
            .map_err(|e| PyIOError::new_err(e.to_string()))?;
        for module in &self.modules {
            let relative = module.path.trim_start_matches("./");
            let path = root.join(PathBuf::from(relative));
            if path.strip_prefix(root).is_err() {
                return Err(PyValueError::new_err(
                    "bundle module path traversal detected",
                ));
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
        format!(
            "Result(code_len={}, bundle={})",
            self.code.len(),
            self.bundle.is_some()
        )
    }
}

#[pyfunction]
#[pyo3(signature = (source, *, filename=None, source_type="auto", minify=false))]
pub fn transform(
    source: &str,
    filename: Option<&str>,
    source_type: &str,
    minify: bool,
) -> PyResult<Result> {
    let source = bookmarklet_source(source);
    let (code, diagnostics) = parse_and_generate(&source, filename, source_type, minify, true)?;
    Ok(Result {
        code,
        bundle: None,
        diagnostics,
    })
}

#[pyfunction]
#[pyo3(signature = (source, *, filename=None, source_type="auto"))]
pub fn format(source: &str, filename: Option<&str>, source_type: &str) -> PyResult<String> {
    Ok(transform(source, filename, source_type, false)?.code)
}

#[pyfunction]
#[pyo3(signature = (source, *, filename=None, source_type="auto"))]
pub fn minify(source: &str, filename: Option<&str>, source_type: &str) -> PyResult<String> {
    let source = bookmarklet_source(source);
    Ok(parse_and_generate(&source, filename, source_type, true, true)?.0)
}

#[pyfunction]
#[pyo3(signature = (source, *, filename=None, source_type="auto"))]
pub fn unminify(source: &str, filename: Option<&str>, source_type: &str) -> PyResult<String> {
    let source = bookmarklet_source(source);
    Ok(parse_and_generate(&source, filename, source_type, false, true)?.0)
}

#[pyfunction]
#[pyo3(signature = (source, *, filename=None, source_type="auto"))]
pub fn deobfuscate(source: &str, filename: Option<&str>, source_type: &str) -> PyResult<String> {
    let source = deobfuscate_utils::strip_debugger_statements(&bookmarklet_source(source));
    let source = deobfuscate_utils::static_deobfuscate(&source);
    unminify(&source, filename, source_type)
}

#[pyfunction]
#[pyo3(signature = (source))]
pub fn unpack(source: &str) -> PyResult<Option<Bundle>> {
    let normalized = bookmarklet_source(source);
    let (bundle_type, entry_id) = match detect_bundle(&normalized) {
        Some(value) => value,
        None => return Ok(None),
    };
    let (code, _) = parse_and_generate(&normalized, None, "auto", false, false)?;
    Ok(Some(Bundle {
        bundle_type,
        entry_id,
        modules: vec![Module {
            id: "0".to_string(),
            path: "./index.js".to_string(),
            code,
            is_entry: true,
        }],
    }))
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
        if let Some(value) = opts.get_item("unminify")? {
            unminify = value.extract()?;
        }
        if let Some(value) = opts.get_item("unpack")? {
            unpack = value.extract()?;
        }
        if let Some(value) = opts.get_item("deobfuscate")? {
            deobfuscate = value.extract()?;
        }
        if let Some(value) = opts.get_item("mangle")? {
            mangle = value.extract()?;
        }
    }
    let normalized = bookmarklet_source(source);
    let normalized = if deobfuscate {
        deobfuscate_utils::static_deobfuscate(&normalized)
    } else {
        normalized
    };
    let (code, diagnostics) = parse_and_generate(&normalized, None, "auto", mangle, unminify)?;
    let bundle = if unpack && deobfuscate {
        detect_bundle(&normalized).map(|(bundle_type, entry_id)| Bundle {
            bundle_type,
            entry_id,
            modules: vec![Module {
                id: "0".to_string(),
                path: "./index.js".to_string(),
                code: code.clone(),
                is_entry: true,
            }],
        })
    } else {
        None
    };
    Ok(Result {
        code,
        bundle,
        diagnostics,
    })
}

#[pymodule]
fn _native(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<Module>()?;
    m.add_class::<Bundle>()?;
    m.add_class::<Result>()?;
    m.add_function(wrap_pyfunction!(transform, m)?)?;
    m.add_function(wrap_pyfunction!(format, m)?)?;
    m.add_function(wrap_pyfunction!(minify, m)?)?;
    m.add_function(wrap_pyfunction!(unminify, m)?)?;
    m.add_function(wrap_pyfunction!(deobfuscate, m)?)?;
    m.add_function(wrap_pyfunction!(unpack, m)?)?;
    m.add_function(wrap_pyfunction!(webcrack, m)?)?;
    let _ = PyList::empty(m.py());
    Ok(())
}
