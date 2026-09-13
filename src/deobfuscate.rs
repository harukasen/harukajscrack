//! Conservative, non-executing deobfuscation helpers.
//! Upstream webcrack uses isolated-vm for arbitrary decoder execution; this
//! module intentionally never evaluates untrusted JavaScript.

pub fn decode_hex_escape(input: &str) -> Option<String> {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' && chars.peek() == Some(&'x') {
            chars.next();
            let hi = chars.next()?.to_digit(16)?;
            let lo = chars.next()?.to_digit(16)?;
            out.push(char::from_u32(hi * 16 + lo)?);
        } else if c == '\\' && chars.peek() == Some(&'u') {
            chars.next();
            let mut value = 0u32;
            for _ in 0..4 { value = value * 16 + chars.next()?.to_digit(16)?; }
            out.push(char::from_u32(value)?);
        } else { out.push(c); }
    }
    Some(out)
}

pub fn strip_debugger_statements(source: &str) -> String {
    source.lines().map(|line| {
        if line.trim() == "debugger;" || line.trim() == "debugger" { "" } else { line }
    }).collect::<Vec<_>>().join("\n")
}

pub fn is_static_string(value: &str) -> bool {
    let trimmed = value.trim();
    trimmed.len() >= 2 && ((trimmed.starts_with('"') && trimmed.ends_with('"')) || (trimmed.starts_with('\'') && trimmed.ends_with('\'')))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn decodes_escapes() { assert_eq!(decode_hex_escape(r"A\x42\u0043"), Some("ABC".into())); }
    #[test] fn removes_debugger_lines() { assert_eq!(strip_debugger_statements("a();\ndebugger;\nb();"), "a();\n\nb();"); }
}
