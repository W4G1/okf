//! Block-style YAML emitter for the OKF subset.
//!
//! The emitter targets one property: re-parsing its output reproduces the input
//! value (`parse(emit(v)) == v`), with mapping key order preserved. It is not
//! intended to be byte-identical to any other YAML writer.

use super::{Mapping, Value};
use std::fmt::Write as _;

const INDENT_STEP: usize = 2;

/// YAML indicators a plain scalar must not begin with (`yaml.org/1.2 section 4.2.2`).
const INDICATORS: &[char] = &[
    '-', '?', ':', ',', '[', ']', '{', '}', '#', '&', '*', '!', '|', '>', '\'', '"', '%', '@', '`',
    ' ',
];

/// Emits a value as YAML text (always ends with a newline, like `PyYAML`'s
/// `safe_dump`).
pub fn emit(value: &Value) -> String {
    let mut out = String::new();
    match value {
        Value::Mapping(m) if !m.is_empty() => emit_mapping(m, 0, &mut out),
        Value::Sequence(s) if !s.is_empty() => emit_sequence(s, 0, &mut out),
        scalar => {
            out.push_str(&emit_scalar(scalar));
            out.push('\n');
        }
    }
    out
}

fn emit_mapping(map: &Mapping, indent: usize, out: &mut String) {
    let pad = " ".repeat(indent);
    for (k, v) in map.iter() {
        let key = emit_scalar(k);
        match v {
            Value::Mapping(m) if !m.is_empty() => {
                let _ = writeln!(out, "{pad}{key}:");
                emit_mapping(m, indent + INDENT_STEP, out);
            }
            Value::Sequence(s) if !s.is_empty() => {
                let _ = writeln!(out, "{pad}{key}:");
                emit_sequence(s, indent + INDENT_STEP, out);
            }
            _ => {
                let _ = writeln!(out, "{pad}{key}: {}", emit_scalar(v));
            }
        }
    }
}

fn emit_sequence(seq: &[Value], indent: usize, out: &mut String) {
    let pad = " ".repeat(indent);
    for item in seq {
        match item {
            Value::Mapping(m) if !m.is_empty() => {
                let mut block = String::new();
                emit_mapping(m, indent + INDENT_STEP, &mut block);
                out.push_str(&bullet(&block, indent));
            }
            Value::Sequence(s) if !s.is_empty() => {
                let mut block = String::new();
                emit_sequence(s, indent + INDENT_STEP, &mut block);
                out.push_str(&bullet(&block, indent));
            }
            _ => {
                let _ = writeln!(out, "{pad}- {}", emit_scalar(item));
            }
        }
    }
}

/// Turns an indented block into a sequence item by replacing the first line's
/// indentation with a `- ` marker, so a collection item reads
/// `- name: year` rather than a bare `-` followed by the block. The marker is
/// exactly as wide as the indentation it replaces, so the remaining lines stay
/// aligned.
fn bullet(block: &str, indent: usize) -> String {
    block
        .strip_prefix(&" ".repeat(indent + INDENT_STEP))
        .map_or_else(
            || block.to_string(),
            |rest| format!("{}- {rest}", " ".repeat(indent)),
        )
}

/// Emits a scalar (or an empty collection) inline.
fn emit_scalar(value: &Value) -> String {
    match value {
        Value::Null => "null".to_string(),
        Value::Bool(true) => "true".to_string(),
        Value::Bool(false) => "false".to_string(),
        Value::Int(i) => i.to_string(),
        Value::Float(f) => format_float(*f),
        Value::String(s) => emit_string(s),
        Value::Sequence(s) if s.is_empty() => "[]".to_string(),
        Value::Mapping(m) if m.is_empty() => "{}".to_string(),
        // Non-empty collections never reach here in block context.
        Value::Sequence(_) | Value::Mapping(_) => "[]".to_string(),
    }
}

pub(super) fn format_float(f: f64) -> String {
    if f.is_nan() {
        return ".nan".to_string();
    }
    if f.is_infinite() {
        return if f > 0.0 {
            ".inf".to_string()
        } else {
            "-.inf".to_string()
        };
    }
    // `{:?}` is the shortest round-tripping representation, but it can omit the
    // decimal point for exponential forms (`1e30`). Ensure a `.` is present so
    // the value re-parses as a float rather than a string.
    let s = format!("{f:?}");
    if s.contains('.') {
        s
    } else if let Some(e) = s.find(['e', 'E']) {
        format!("{}.0{}", &s[..e], &s[e..])
    } else {
        format!("{s}.0")
    }
}

fn emit_string(s: &str) -> String {
    if is_safe_plain(s) {
        s.to_string()
    } else {
        double_quote(s)
    }
}

/// Whether a string can be emitted as a plain (unquoted) scalar without being
/// misread on re-parse.
fn is_safe_plain(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    // Must not be reinterpreted as null/bool/number.
    if super::Value::parse(s).map_or(true, |v| v != Value::String(s.to_string())) {
        // parse() of a multiline/odd string may error; fall through to quoting.
        return false;
    }
    if s.starts_with(' ') || s.ends_with(' ') {
        return false;
    }
    if resolves_as_datetime(s) || resolves_elsewhere(s) {
        return false;
    }
    let first = s.chars().next().unwrap();
    if INDICATORS.contains(&first) {
        return false;
    }
    let bytes: Vec<char> = s.chars().collect();
    for (i, &c) in bytes.iter().enumerate() {
        match c {
            '\n' | '\t' | '\r' => return false,
            ':' if bytes.get(i + 1).is_none_or(|n| *n == ' ') => return false,
            '#' if i > 0 && bytes[i - 1] == ' ' => return false,
            _ => {}
        }
    }
    true
}

/// Whether another YAML reader would resolve this plain scalar to something
/// other than a string, even though this parser keeps it as one.
///
/// This parser resolves numbers conservatively, so `007`, `0x1F`, `1e3`, `no`,
/// and `12:30` all read back as the strings they are. A YAML 1.1 reader such
/// as `PyYAML`, which the reference implementation uses, reads them as 7, 31,
/// 1000.0, false, and 750; a YAML 1.2 core-schema reader agrees on some. A
/// string must stay a string for every reader, so these are quoted. The
/// specification asks the same of producers: quote values that look like
/// numbers or booleans.
///
/// The forms are those of the YAML 1.1 and 1.2 implicit resolvers: the
/// boolean and null words, `=` and `<<`, integers in any radix with `_`
/// separators, floats with or without an exponent, and base-60 numbers.
fn resolves_elsewhere(s: &str) -> bool {
    const WORDS: [&str; 28] = [
        "y", "Y", "yes", "Yes", "YES", "n", "N", "no", "No", "NO", "true", "True", "TRUE", "false",
        "False", "FALSE", "on", "On", "ON", "off", "Off", "OFF", "~", "null", "Null", "NULL", "=",
        "<<",
    ];
    if WORDS.contains(&s) {
        return true;
    }
    let body = s.strip_prefix(['+', '-']).unwrap_or(s);
    if matches!(body, ".inf" | ".Inf" | ".INF" | ".nan" | ".NaN" | ".NAN") {
        return true;
    }
    let all = |digits: &str, valid: fn(char) -> bool| {
        !digits.is_empty() && digits.chars().all(|c| valid(c) || c == '_')
    };
    if let Some(hex) = body.strip_prefix("0x") {
        return all(hex, |c| c.is_ascii_hexdigit());
    }
    if let Some(octal) = body.strip_prefix("0o") {
        return all(octal, |c| ('0'..='7').contains(&c));
    }
    if let Some(binary) = body.strip_prefix("0b") {
        return all(binary, |c| c == '0' || c == '1');
    }

    let (mantissa, exponent) = body
        .find(['e', 'E'])
        .map_or((body, None), |i| (&body[..i], Some(&body[i + 1..])));
    if let Some(exponent) = exponent {
        let digits = exponent.strip_prefix(['+', '-']).unwrap_or(exponent);
        if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit()) {
            return false;
        }
    }
    if !mantissa.starts_with(|c: char| c.is_ascii_digit() || c == '.') {
        return false;
    }
    let mut dots = 0;
    let mut has_digit = false;
    for c in mantissa.chars() {
        match c {
            '0'..='9' => has_digit = true,
            '.' => dots += 1,
            // `_` separates digit groups, and `:` separates base-60 places.
            '_' | ':' => {}
            _ => return false,
        }
    }
    has_digit && dots <= 1
}

/// Whether YAML's implicit resolver would type this plain scalar as a timestamp
/// carrying a time of day rather than as a string.
///
/// This parser keeps every timestamp as a string, so nothing here depends on the
/// distinction, but `PyYAML` (which the reference implementation loads and dumps
/// with) does not. The reference writes `generated.at` and `verified[].at` with
/// `datetime.isoformat()`, making them Python strings, and `safe_dump` quotes
/// them so a loader cannot retype them. Emitting one bare would silently turn a
/// string into a `datetime` for any `PyYAML` consumer, so it is quoted here too.
///
/// A bare `YYYY-MM-DD` is deliberately left plain: that is how both the
/// specification and the reference write `stale_after`, `last_modified`, and
/// `usage_window`, and quoting it would be the same fidelity loss in reverse.
/// The grammar below is the datetime half of `PyYAML`'s own timestamp pattern.
fn resolves_as_datetime(s: &str) -> bool {
    let b = s.as_bytes();
    let mut i = 0;

    // Date: YYYY-M(M)-D(D).
    if !(take_digits(b, &mut i, 4, 4)
        && take_byte(b, &mut i, b'-')
        && take_digits(b, &mut i, 1, 2)
        && take_byte(b, &mut i, b'-')
        && take_digits(b, &mut i, 1, 2))
    {
        return false;
    }

    // Date/time separator: `T`, `t`, or a run of blanks.
    match b.get(i) {
        Some(b'T' | b't') => i += 1,
        Some(b' ' | b'\t') => {
            while matches!(b.get(i), Some(b' ' | b'\t')) {
                i += 1;
            }
        }
        _ => return false,
    }

    // Time: H(H):MM:SS with optional fractional seconds.
    if !(take_digits(b, &mut i, 1, 2)
        && take_byte(b, &mut i, b':')
        && take_digits(b, &mut i, 2, 2)
        && take_byte(b, &mut i, b':')
        && take_digits(b, &mut i, 2, 2))
    {
        return false;
    }
    if take_byte(b, &mut i, b'.') {
        take_digits(b, &mut i, 0, usize::MAX);
    }

    // Optional zone: `Z` or +/-HH(:MM). Blanks count only when one follows.
    let before_blanks = i;
    while matches!(b.get(i), Some(b' ' | b'\t')) {
        i += 1;
    }
    if i == b.len() {
        return before_blanks == i;
    }
    if !take_byte(b, &mut i, b'Z') {
        if !(take_byte(b, &mut i, b'+') || take_byte(b, &mut i, b'-')) {
            return false;
        }
        if !take_digits(b, &mut i, 1, 2) {
            return false;
        }
        if take_byte(b, &mut i, b':') && !take_digits(b, &mut i, 2, 2) {
            return false;
        }
    }
    i == b.len()
}

/// Consumes up to `max` ASCII digits, reporting whether at least `min` were
/// there.
fn take_digits(b: &[u8], i: &mut usize, min: usize, max: usize) -> bool {
    let start = *i;
    while *i - start < max && b.get(*i).is_some_and(u8::is_ascii_digit) {
        *i += 1;
    }
    *i - start >= min
}

/// Consumes `want` when it is the next byte.
fn take_byte(b: &[u8], i: &mut usize, want: u8) -> bool {
    let found = b.get(*i) == Some(&want);
    if found {
        *i += 1;
    }
    found
}

fn double_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            '\u{0008}' => out.push_str("\\b"),
            '\u{000C}' => out.push_str("\\f"),
            '\0' => out.push_str("\\0"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
