//! Small, dependency-light equivalents of upstream webcrack's AST utilities.
//! AST mutation passes remain in the Oxc visitor in `lib.rs`; these helpers are
//! shared by parsing, deobfuscation, and bundle extraction.

use std::path::{Component, Path};

pub fn normalize_bookmarklet(source: &str) -> String {
    source.strip_prefix("javascript:").unwrap_or(source).to_string()
}

pub fn is_safe_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c == '_' || c == '$' || c.is_ascii_alphabetic() => {}
        _ => return false,
    }
    chars.all(|c| c == '_' || c == '$' || c.is_ascii_alphanumeric())
}

pub fn relative_module_path(from: &str, to: &str) -> String {
    let from_dir = Path::new(from).parent().unwrap_or_else(|| Path::new("."));
    let from_parts: Vec<_> = from_dir.components().collect();
    let to_parts: Vec<_> = Path::new(to).components().collect();
    let mut common = 0;
    while common < from_parts.len() && common < to_parts.len() && from_parts[common] == to_parts[common] {
        common += 1;
    }
    let mut result = Vec::new();
    for component in &from_parts[common..] {
        if !matches!(component, Component::CurDir) { result.push(".."); }
    }
    for component in &to_parts[common..] {
        if let Component::Normal(value) = component { result.push(value.to_str().unwrap_or_default()); }
    }
    if result.is_empty() { "./".to_string() }
    else if result[0] == ".." { result.join("/") }
    else { format!("./{}", result.join("/")) }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn identifiers_are_checked() { assert!(is_safe_identifier("module_1")); assert!(!is_safe_identifier("1module")); }
    #[test] fn paths_are_relative() { assert_eq!(relative_module_path("./a/index.js", "./b.js"), "../b.js"); }
}
