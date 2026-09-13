//! Conservative, non-executing deobfuscation helpers.
//!
//! This module intentionally never evaluates JavaScript.  The static pass only
//! handles literal arrays, literal numeric index arithmetic, and decoder
//! functions whose result is a read from such an array.  It is deliberately
//! narrower than upstream webcrack's isolated-vm based pass.

use std::collections::HashMap;

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
            for _ in 0..4 {
                value = value * 16 + chars.next()?.to_digit(16)?;
            }
            out.push(char::from_u32(value)?);
        } else {
            out.push(c);
        }
    }
    Some(out)
}

pub fn strip_debugger_statements(source: &str) -> String {
    source
        .lines()
        .map(|line| {
            if line.trim() == "debugger;" || line.trim() == "debugger" {
                ""
            } else {
                line
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn is_static_string(value: &str) -> bool {
    let trimmed = value.trim();
    trimmed.len() >= 2
        && ((trimmed.starts_with('"') && trimmed.ends_with('"'))
            || (trimmed.starts_with('\'') && trimmed.ends_with('\'')))
}

#[derive(Clone, Debug)]
enum Literal {
    String(String),
    Undefined,
}

fn is_ident(c: u8) -> bool {
    c == b'_' || c == b'$' || c.is_ascii_alphanumeric()
}
fn boundary(source: &[u8], start: usize, end: usize) -> bool {
    (start == 0 || !is_ident(source[start - 1])) && (end >= source.len() || !is_ident(source[end]))
}
fn skip_ws(source: &[u8], mut i: usize) -> usize {
    while i < source.len() {
        if source[i].is_ascii_whitespace() {
            i += 1;
        } else if i + 1 < source.len() && source[i] == b'/' && source[i + 1] == b'/' {
            i += 2;
            while i < source.len() && source[i] != b'\n' {
                i += 1;
            }
        } else if i + 1 < source.len() && source[i] == b'/' && source[i + 1] == b'*' {
            i += 2;
            while i + 1 < source.len() && !(source[i] == b'*' && source[i + 1] == b'/') {
                i += 1;
            }
            i = (i + 2).min(source.len());
        } else {
            break;
        }
    }
    i
}
fn matching(source: &[u8], open: usize, left: u8, right: u8) -> Option<usize> {
    let mut depth = 0;
    let mut i = open;
    let mut quote = 0;
    let mut escaped = false;
    while i < source.len() {
        let c = source[i];
        if quote != 0 {
            if escaped {
                escaped = false;
            } else if c == b'\\' {
                escaped = true;
            } else if c == quote {
                quote = 0;
            }
            i += 1;
            continue;
        }
        if c == b'\'' || c == b'"' || c == b'`' {
            quote = c;
            i += 1;
            continue;
        }
        if i + 1 < source.len() && c == b'/' && source[i + 1] == b'/' {
            i += 2;
            while i < source.len() && source[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if i + 1 < source.len() && c == b'/' && source[i + 1] == b'*' {
            i += 2;
            while i + 1 < source.len() && !(source[i] == b'*' && source[i + 1] == b'/') {
                i += 1;
            }
            i += 2;
            continue;
        }
        if c == left {
            depth += 1;
        } else if c == right {
            depth -= 1;
            if depth == 0 {
                return Some(i);
            }
        }
        i += 1;
    }
    None
}
fn split_top(source: &str) -> Vec<&str> {
    let b = source.as_bytes();
    let mut out = Vec::new();
    let mut start = 0;
    let mut i = 0;
    let mut stack = Vec::new();
    let mut quote = 0;
    let mut esc = false;
    while i < b.len() {
        let c = b[i];
        if quote != 0 {
            if esc {
                esc = false
            } else if c == b'\\' {
                esc = true
            } else if c == quote {
                quote = 0
            };
            i += 1;
            continue;
        }
        if c == b'\'' || c == b'"' {
            quote = c;
            i += 1;
            continue;
        }
        match c {
            b'(' | b'[' | b'{' => stack.push(c),
            b')' => {
                if stack.last() == Some(&b'(') {
                    stack.pop();
                }
            }
            b']' => {
                if stack.last() == Some(&b'[') {
                    stack.pop();
                }
            }
            b'}' => {
                if stack.last() == Some(&b'{') {
                    stack.pop();
                }
            }
            b',' if stack.is_empty() => {
                out.push(source[start..i].trim());
                start = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    out.push(source[start..].trim());
    out
}
fn parse_string(s: &str) -> Option<String> {
    let s = s.trim();
    let b = s.as_bytes();
    if b.len() < 2
        || !((b[0] == b'\'' && b[b.len() - 1] == b'\'') || (b[0] == b'"' && b[b.len() - 1] == b'"'))
    {
        return None;
    }
    let mut out = String::new();
    let mut i = 1;
    while i + 1 < b.len() {
        let c = b[i];
        if c != b'\\' {
            out.push(c as char);
            i += 1;
            continue;
        }
        i += 1;
        if i + 1 >= b.len() {
            return None;
        }
        let e = b[i];
        match e {
            b'n' => out.push('\n'),
            b'r' => out.push('\r'),
            b't' => out.push('\t'),
            b'b' => out.push('\x08'),
            b'f' => out.push('\x0c'),
            b'v' => out.push('\x0b'),
            b'0' => out.push('\0'),
            b'x' => {
                if i + 2 >= b.len() {
                    return None;
                };
                let n =
                    u32::from_str_radix(std::str::from_utf8(&b[i + 1..i + 3]).ok()?, 16).ok()?;
                out.push(char::from_u32(n)?);
                i += 2;
            }
            b'u' => {
                if i + 4 >= b.len() {
                    return None;
                };
                let n =
                    u32::from_str_radix(std::str::from_utf8(&b[i + 1..i + 5]).ok()?, 16).ok()?;
                out.push(char::from_u32(n)?);
                i += 4;
            }
            b'\'' => out.push('\''),
            b'"' => out.push('"'),
            b'\\' => out.push('\\'),
            _ => out.push(e as char),
        }
        i += 1;
    }
    Some(out)
}
fn parse_array(s: &str) -> Option<Vec<Literal>> {
    let parts = split_top(s);
    if parts.len() == 1 && parts[0].is_empty() {
        return Some(Vec::new());
    }
    parts
        .into_iter()
        .map(|p| {
            let p = p.trim();
            if p == "void 0" || p == "void(0)" || p == "undefined" {
                Some(Literal::Undefined)
            } else {
                parse_string(p).map(Literal::String)
            }
        })
        .collect()
}
fn token_positions(source: &str, name: &str) -> Vec<usize> {
    let b = source.as_bytes();
    let n = name.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i + n.len() <= b.len() {
        if &b[i..i + n.len()] == n && boundary(b, i, i + n.len()) {
            out.push(i);
            i += n.len();
        } else {
            i += 1;
        }
    }
    out
}
fn js_number(s: &str) -> Option<f64> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    };
    let neg = s.starts_with('-');
    let t = if neg { s[1..].trim() } else { s };
    let n = if t.starts_with("0x") || t.starts_with("0X") {
        u64::from_str_radix(&t[2..], 16).ok()? as f64
    } else if t.starts_with("0b") || t.starts_with("0B") {
        u64::from_str_radix(&t[2..], 2).ok()? as f64
    } else {
        t.parse::<f64>().ok()?
    };
    Some(if neg { -n } else { n })
}

// A tiny arithmetic evaluator. It accepts only numeric literals, identifiers
// supplied by the caller, parentheses, and + - * / % bitwise operators.
fn eval_num(s: &str, vars: &HashMap<String, f64>) -> Option<f64> {
    struct P<'a> {
        b: &'a [u8],
        i: usize,
        v: &'a HashMap<String, f64>,
    }
    impl<'a> P<'a> {
        fn ws(&mut self) {
            while self.i < self.b.len() && self.b[self.i].is_ascii_whitespace() {
                self.i += 1
            }
        }
        fn primary(&mut self) -> Option<f64> {
            self.ws();
            if self.i >= self.b.len() {
                return None;
            };
            if self.b[self.i] == b'(' {
                self.i += 1;
                let x = self.add()?;
                self.ws();
                if self.b.get(self.i) != Some(&b')') {
                    return None;
                };
                self.i += 1;
                return Some(x);
            };
            let st = self.i;
            if self.b[self.i] == b'-' {
                self.i += 1;
                let x = self.primary()?;
                return Some(-x);
            };
            while self.i < self.b.len()
                && (self.b[self.i].is_ascii_alphanumeric()
                    || self.b[self.i] == b'.'
                    || self.b[self.i] == b'_')
            {
                self.i += 1
            }
            let t = std::str::from_utf8(&self.b[st..self.i]).ok()?;
            if let Some(x) = self.v.get(t) {
                Some(*x)
            } else {
                js_number(t)
            }
        }
        fn mul(&mut self) -> Option<f64> {
            let mut x = self.primary()?;
            loop {
                self.ws();
                let op = self.b.get(self.i).copied();
                if !matches!(op, Some(b'*') | Some(b'/') | Some(b'%')) {
                    break;
                }
                self.i += 1;
                let y = self.primary()?;
                x = match op.unwrap() {
                    b'*' => x * y,
                    b'/' => x / y,
                    _ => x % y,
                };
            }
            Some(x)
        }
        fn add(&mut self) -> Option<f64> {
            let mut x = self.mul()?;
            loop {
                self.ws();
                let op = self.b.get(self.i).copied();
                if !matches!(op, Some(b'+') | Some(b'-')) {
                    break;
                }
                self.i += 1;
                let y = self.mul()?;
                x = if op == Some(b'+') { x + y } else { x - y };
            }
            Some(x)
        }
    }
    let mut p = P {
        b: s.as_bytes(),
        i: 0,
        v: vars,
    };
    let x = p.add()?;
    p.ws();
    if p.i == p.b.len() {
        Some(x)
    } else {
        None
    }
}

#[derive(Clone)]
struct ArrayDef {
    name: String,
    values: Vec<Literal>,
    start: usize,
    end: usize,
    wrapper: Option<(usize, usize, String)>,
}
fn find_arrays(source: &str) -> Vec<ArrayDef> {
    let b = source.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let kw = if b[i..].starts_with(b"const") {
            "const"
        } else if b[i..].starts_with(b"let") {
            "let"
        } else if b[i..].starts_with(b"var") {
            "var"
        } else {
            i += 1;
            continue;
        };
        let ks = i;
        if !boundary(b, i, i + kw.len()) {
            i += kw.len();
            continue;
        };
        let mut j = skip_ws(b, i + kw.len());
        let ns = j;
        while j < b.len() && is_ident(b[j]) {
            j += 1
        }
        if ns == j {
            i += kw.len();
            continue;
        };
        let name = &source[ns..j];
        j = skip_ws(b, j);
        if b.get(j) != Some(&b'=') {
            i += kw.len();
            continue;
        };
        j = skip_ws(b, j + 1);
        if b.get(j) != Some(&b'[') {
            i += kw.len();
            continue;
        };
        let Some(e) = matching(b, j, b'[', b']') else {
            i += kw.len();
            continue;
        };
        let Some(values) = parse_array(&source[j + 1..e]) else {
            i = e + 1;
            continue;
        };
        let mut wrapper = None;
        let before = &source[..ks];
        if let Some(fp) = before.rfind("function") {
            let mut q = fp + 8;
            q = skip_ws(b, q);
            let fs = q;
            while q < b.len() && is_ident(b[q]) {
                q += 1
            }
            if fs < q {
                let fname = &source[fs..q];
                if let Some(bs) = source[q..].find('{') {
                    let bs = q + bs;
                    if let Some(be) = matching(b, bs, b'{', b'}') {
                        if bs < ks && ks < be {
                            let compact: String = source[bs..be]
                                .chars()
                                .filter(|c| !c.is_ascii_whitespace())
                                .collect();
                            if compact.contains(&format!("{}=function", fname)) {
                                wrapper = Some((fp, be + 1, fname.to_string()));
                            }
                        }
                    }
                }
            }
        }
        out.push(ArrayDef {
            name: name.to_string(),
            values,
            start: ks,
            end: e + 1,
            wrapper,
        });
        i = e + 1;
    }
    out
}
fn ranges_contains(ranges: &[(usize, usize)], p: usize) -> bool {
    ranges.iter().any(|(a, b)| p >= *a && p < *b)
}

/// Apply the safe static subset. No JavaScript is executed; uncertain or
/// mutable arrays are retained unchanged.
pub fn static_deobfuscate(source: &str) -> String {
    let mut out = source.to_string();
    let arrays = find_arrays(source);
    let mut replacements: Vec<(usize, usize, String)> = Vec::new();
    let mut removals: Vec<(usize, usize)> = Vec::new();
    for a in &arrays {
        let mut protected = vec![(a.start, a.end)];
        if let Some((s, e, _)) = a.wrapper {
            protected.push((s, e));
        }
        let positions = token_positions(source, &a.name);
        let mut reads = Vec::new();
        let mut safe = true;
        for p in positions {
            if ranges_contains(&protected, p) {
                continue;
            };
            let mut j = skip_ws(source.as_bytes(), p + a.name.len());
            if source.as_bytes().get(j) != Some(&b'[') {
                safe = false;
                break;
            };
            let Some(e) = matching(source.as_bytes(), j, b'[', b']') else {
                safe = false;
                break;
            };
            let idx = source[j + 1..e].trim();
            let Some(n) = js_number(idx) else {
                safe = false;
                break;
            };
            if n.fract() != 0.0 || n < 0.0 || n as usize >= a.values.len() {
                safe = false;
                break;
            };
            let after = skip_ws(source.as_bytes(), e + 1);
            if source.as_bytes().get(after) == Some(&b'=')
                || source.as_bytes().get(after) == Some(&b'+')
                || source.as_bytes().get(after) == Some(&b'-')
            {
                safe = false;
                break;
            };
            reads.push((p, e + 1, n as usize));
        }
        if safe && !reads.is_empty() && a.wrapper.is_none() {
            for (s, e, n) in reads {
                let v = match &a.values[n] {
                    Literal::String(x) => serde_json::to_string(x).unwrap(),
                    Literal::Undefined => "void 0".into(),
                };
                replacements.push((s, e, v));
            }
            removals.push((a.start, a.end));
        }
    }
    // Discover literal decoders backed by a wrapped array. The decoder body
    // must contain `tmp = arrayFn()` and a computed read from that tmp.
    let mut decoder_removals = Vec::new();
    let mut decoded_any = false;
    for a in arrays.iter().filter(|a| a.wrapper.is_some()) {
        let Some((_, _, array_fn)) = a.wrapper.clone() else {
            continue;
        };
        let b = source.as_bytes();
        let mut scan = 0;
        while let Some(rel) = source[scan..].find("function") {
            let fs = scan + rel;
            let mut q = skip_ws(b, fs + 8);
            let ns = q;
            while q < b.len() && is_ident(b[q]) {
                q += 1
            }
            if ns == q {
                scan = fs + 8;
                continue;
            };
            let dname = &source[ns..q];
            if dname == array_fn {
                scan = q;
                continue;
            };
            let ps = skip_ws(b, q);
            if b.get(ps) != Some(&b'(') {
                scan = q;
                continue;
            };
            let Some(pe) = matching(b, ps, b'(', b')') else {
                break;
            };
            let params: Vec<&str> = split_top(&source[ps + 1..pe]);
            let bs = skip_ws(b, pe + 1);
            if b.get(bs) != Some(&b'{') {
                scan = pe + 1;
                continue;
            };
            let Some(be) = matching(b, bs, b'{', b'}') else {
                break;
            };
            let body = &source[bs + 1..be];
            let Some(callat) = body.find(&format!("()",)) else {
                scan = be + 1;
                continue;
            };
            let _ = callat;
            if !body.contains(&format!("{}()", array_fn)) {
                scan = be + 1;
                continue;
            };
            let mut tmp = None;
            for kw in ["var ", "let ", "const "] {
                if let Some(x) = body.find(kw) {
                    let st = x + kw.len();
                    let mut z = st;
                    while z < body.len() && is_ident(body.as_bytes()[z]) {
                        z += 1
                    }
                    let tn = &body[st..z];
                    let compact_tail: String = body[z..]
                        .chars()
                        .filter(|c| !c.is_ascii_whitespace())
                        .collect();
                    if compact_tail.starts_with(&format!("={}()", array_fn)) {
                        tmp = Some(tn.to_string());
                        break;
                    }
                }
            }
            let Some(tmp) = tmp else {
                scan = be + 1;
                continue;
            };
            let Some(mi) = body.find(&format!("{}[", tmp)) else {
                scan = be + 1;
                continue;
            };
            let ib = bs + 1 + mi + tmp.len() + 1;
            let Some(ie) = matching(b, ib - 1, b'[', b']') else {
                scan = be + 1;
                continue;
            };
            let idx_expr = &source[ib..ie];
            let mut param_idx = None;
            let mut offset = 0.0;
            for (pi, pn) in params.iter().enumerate() {
                let pn = pn.trim();
                if idx_expr.trim().starts_with(pn) {
                    param_idx = Some(pi);
                    let rest = idx_expr.trim()[pn.len()..].trim();
                    if !rest.is_empty() {
                        if let Some(x) = rest.strip_prefix("-") {
                            offset = eval_num(x, &HashMap::new()).unwrap_or(0.0) * -1.0
                        } else if let Some(x) = rest.strip_prefix("+") {
                            offset = eval_num(x, &HashMap::new()).unwrap_or(0.0)
                        }
                    }
                    break;
                }
            }
            let Some(pi) = param_idx else {
                scan = be + 1;
                continue;
            };
            if offset == 0.0 {
                let pn = params[pi].trim();
                let compact_body: String =
                    body.chars().filter(|c| !c.is_ascii_whitespace()).collect();
                let marker = format!("{}={}-", pn, pn);
                if let Some(ai) = compact_body.find(&marker) {
                    let tail = &compact_body[ai + marker.len()..];
                    let rhs = tail.split([';', ',', ')', ']']).next().unwrap_or("");
                    if let Some(v) = eval_num(rhs, &HashMap::new()) {
                        offset = -v;
                    }
                }
            }
            let calls = token_positions(source, dname);
            let mut replaced = false;
            for cp in calls {
                if cp >= fs && cp <= be {
                    continue;
                };
                let ca = skip_ws(b, cp + dname.len());
                if b.get(ca) != Some(&b'(') {
                    continue;
                };
                let Some(ce) = matching(b, ca, b'(', b')') else {
                    continue;
                };
                let args = split_top(&source[ca + 1..ce]);
                if pi >= args.len() {
                    continue;
                };
                let Some(arg) = js_number(args[pi]) else {
                    continue;
                };
                let index = (arg + offset) as isize;
                if index < 0 || index as usize >= a.values.len() {
                    continue;
                };
                let v = match &a.values[index as usize] {
                    Literal::String(x) => serde_json::to_string(x).unwrap(),
                    Literal::Undefined => "void 0".into(),
                };
                replacements.push((cp, ce + 1, v));
                replaced = true;
                decoded_any = true;
            }
            if replaced {
                decoder_removals.push((fs, be + 1));
            }
            scan = be + 1;
        }
    }
    if decoded_any {
        for a in arrays.iter().filter(|a| a.wrapper.is_some()) {
            if let Some((s, e, _)) = a.wrapper {
                removals.push((s, e));
            }
        }
        removals.extend(decoder_removals);
    }
    replacements.sort_by_key(|x| x.0);
    removals.sort_by_key(|x| x.0);
    let mut all: Vec<(usize, usize, Option<String>)> = Vec::new();
    for (s, e, v) in replacements {
        all.push((s, e, Some(v)))
    }
    for (s, e) in removals {
        all.push((s, e, None))
    }
    all.sort_by_key(|x| x.0);
    for (s, e, v) in all.into_iter().rev() {
        if s < e && e <= out.len() {
            out.replace_range(s..e, v.as_deref().unwrap_or(""));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn decodes_escapes() {
        assert_eq!(decode_hex_escape(r"A\x42\u0043"), Some("ABC".into()));
    }
    #[test]
    fn removes_debugger_lines() {
        assert_eq!(
            strip_debugger_statements("a();\ndebugger;\nb();"),
            "a();\n\nb();"
        );
    }
    #[test]
    fn simple_array_is_inlined_and_mutation_is_preserved() {
        let x = static_deobfuscate(
            "const a=['log','hi']; console[a[0]](a[1]); const b=['x']; b[0]='y'; console[b[0]]();",
        );
        assert!(x.contains("console[\"log\"](\"hi\")"));
        assert!(x.contains("b[0]=\'y\'"));
    }
    #[test]
    fn wrapped_decoder_is_static() {
        let x=static_deobfuscate("function a(){var x=['log','hi']; a=function(){return x;}; return a();} function d(n){var x=a(); n=n-1; return x[n];} console.log(d(1));");
        assert!(x.contains("console.log(\"log\")"));
        assert!(!x.contains("function d"));
    }
}
