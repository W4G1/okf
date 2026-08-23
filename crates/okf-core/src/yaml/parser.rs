//! Recursive parser for the OKF YAML subset. See the [module docs](super) for
//! the supported grammar and intentional limitations.

use super::{Mapping, Value};
use std::fmt;

/// An error produced while parsing YAML frontmatter.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct YamlError {
    /// 1-based source line where the problem was detected (0 if not known).
    pub line: usize,
    /// Human-readable description.
    pub message: String,
}

impl fmt::Display for YamlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.line > 0 {
            write!(f, "YAML error at line {}: {}", self.line, self.message)
        } else {
            write!(f, "YAML error: {}", self.message)
        }
    }
}

impl std::error::Error for YamlError {}

/// Parses a YAML document (the OKF subset) into a [`Value`].
///
/// Empty or comment/whitespace-only input parses to [`Value::Null`], mirroring
/// `PyYAML`'s `safe_load("") is None`.
pub fn parse(text: &str) -> Result<Value, YamlError> {
    let lines: Vec<String> = text.lines().map(std::string::ToString::to_string).collect();
    let mut p = Parser { lines, pos: 0 };
    p.skip_blank_and_comments();
    if p.pos >= p.lines.len() {
        return Ok(Value::Null);
    }
    let base = p.current_indent()?;
    let value = p.parse_node(base)?;
    p.skip_blank_and_comments();
    if p.pos < p.lines.len() {
        return Err(p.err("unexpected trailing content"));
    }
    Ok(value)
}

struct Parser {
    lines: Vec<String>,
    pos: usize,
}

impl Parser {
    fn err(&self, msg: impl Into<String>) -> YamlError {
        YamlError {
            line: self.pos + 1,
            message: msg.into(),
        }
    }

    fn is_blank_or_comment(line: &str) -> bool {
        let t = line.trim_start();
        t.is_empty() || t.starts_with('#')
    }

    fn skip_blank_and_comments(&mut self) {
        while self.pos < self.lines.len() && Self::is_blank_or_comment(&self.lines[self.pos]) {
            self.pos += 1;
        }
    }

    /// Indentation (count of leading spaces) of the current line. Errors if the
    /// leading whitespace contains a tab (YAML forbids tab indentation).
    fn current_indent(&self) -> Result<usize, YamlError> {
        indent_of(&self.lines[self.pos]).ok_or_else(|| self.err("tab character in indentation"))
    }

    /// Parses a node whose block items begin at column `indent`.
    fn parse_node(&mut self, indent: usize) -> Result<Value, YamlError> {
        let line = &self.lines[self.pos];
        let content = &line[indent.min(line.len())..];
        let trimmed = content.trim_start();

        if trimmed == "-" || trimmed.starts_with("- ") {
            self.parse_sequence(indent)
        } else if split_key_value(trimmed).is_some() {
            self.parse_mapping(indent)
        } else {
            // A bare scalar or flow collection. A plain scalar may continue on
            // the following lines at this same indent.
            let first = trimmed.to_string();
            let entry_line = self.pos;
            self.pos += 1;
            self.read_scalar(&first, indent, entry_line)
        }
    }

    fn parse_mapping(&mut self, indent: usize) -> Result<Value, YamlError> {
        let mut map = Mapping::new();
        loop {
            self.skip_blank_and_comments();
            if self.pos >= self.lines.len() {
                break;
            }
            let ind = self.current_indent()?;
            if ind < indent {
                break;
            }
            if ind > indent {
                return Err(self.err("unexpected indentation in mapping"));
            }
            let line = self.lines[self.pos].clone();
            let content = line[indent..].to_string();
            let trimmed = content.trim_start();
            if trimmed == "-" || trimmed.starts_with("- ") {
                break; // sequence at the same level: not part of this mapping
            }
            let (key_str, rest) = split_key_value(trimmed)
                .ok_or_else(|| self.err("expected 'key: value' mapping entry"))?;
            let key = parse_scalar(&key_str, self.pos)?;
            let entry_line = self.pos;
            self.pos += 1;

            let value = match rest {
                Some(r) if r.starts_with('|') || r.starts_with('>') => {
                    self.parse_block_scalar(indent, &r)?
                }
                Some(r) => self.read_scalar(&r, indent + 1, entry_line)?,
                None => {
                    // Nested block on the following more-indented lines, else null.
                    self.parse_nested(indent)?
                }
            };
            map.push_raw(key, value);
        }
        Ok(Value::Mapping(map))
    }

    fn parse_sequence(&mut self, indent: usize) -> Result<Value, YamlError> {
        let mut seq = Vec::new();
        loop {
            self.skip_blank_and_comments();
            if self.pos >= self.lines.len() {
                break;
            }
            let ind = self.current_indent()?;
            if ind < indent {
                break;
            }
            if ind > indent {
                return Err(self.err("unexpected indentation in sequence"));
            }
            let line = self.lines[self.pos].clone();
            let content = &line[indent..];
            if !(content == "-" || content.starts_with("- ")) {
                break;
            }
            // Column at which the item payload starts.
            let dash_rest = &content[1..]; // after '-'
            let item_offset = indent + 1 + (dash_rest.len() - dash_rest.trim_start().len());
            let item_text = content[1..].trim_start().to_string();
            let entry_line = self.pos;

            if item_text.is_empty() {
                // Nested block belonging to this item.
                self.pos += 1;
                let v = self.parse_nested(indent)?;
                seq.push(v);
            } else if item_text.starts_with('|') || item_text.starts_with('>') {
                self.pos += 1;
                let v = self.parse_block_scalar(indent, &item_text)?;
                seq.push(v);
            } else if split_key_value(&item_text).is_some() {
                // Inline-started mapping element ("- key: value"). Rewrite the
                // dash to whitespace so the payload aligns at `item_offset`,
                // then parse a mapping at that deeper indent.
                let mut rewritten = " ".repeat(item_offset);
                rewritten.push_str(&item_text);
                self.lines[entry_line] = rewritten;
                let v = self.parse_mapping(item_offset)?;
                seq.push(v);
            } else {
                self.pos += 1;
                let v = self.read_scalar(&item_text, indent + 1, entry_line)?;
                seq.push(v);
            }
        }
        Ok(Value::Sequence(seq))
    }

    /// Reads a scalar that starts as `first` and may continue on the following
    /// lines, folding each line break into a single space.
    ///
    /// YAML lets both plain and quoted scalars span lines (line folding),
    /// and `PyYAML`'s `safe_dump` leans on it: any value longer than its 80-column
    /// line width comes out wrapped. The reference implementation dumps with
    /// `safe_dump`, so its own published bundles carry wrapped `description` and
    /// `title` values, and a parser that rejects them cannot read
    /// reference-produced OKF at all.
    ///
    /// A following line continues the scalar when it reaches `min_indent` and,
    /// outside an open quote, does not itself open a mapping entry, a sequence
    /// item, or a comment. Inside an unclosed quote every line belongs to the
    /// scalar until the closing quote, since a wrapped quoted value may contain
    /// anything, `key: value` shapes included. Flow collections are returned as
    /// they stand: nothing emits one across lines.
    fn read_scalar(
        &mut self,
        first: &str,
        min_indent: usize,
        line: usize,
    ) -> Result<Value, YamlError> {
        if first.starts_with('[') || first.starts_with('{') {
            return parse_inline_value(first, line);
        }

        let mut text = first.to_string();
        let mut open_quote = unclosed_quote(first);
        while self.pos < self.lines.len() {
            let raw = &self.lines[self.pos];
            if raw.trim().is_empty() {
                break;
            }
            let ind = indent_of(raw).ok_or_else(|| self.err("tab character in indentation"))?;
            if ind < min_indent {
                break;
            }
            let content = raw.trim();
            if open_quote.is_none() && starts_a_new_node(content) {
                break;
            }
            text.push(' ');
            text.push_str(content);
            if let Some(quote) = open_quote
                && closes_quote(content, quote, 0)
            {
                open_quote = None;
            }
            self.pos += 1;
        }

        parse_scalar(&text, line)
    }

    /// Parses a nested block following a `key:` with no inline value.
    ///
    /// A nested *mapping* must be indented deeper than `parent_indent`. A nested
    /// block *sequence*, however, is also permitted at exactly `parent_indent`
    /// which is YAML's standard "indentation-relaxed" block sequence, and it is
    /// what `PyYAML`'s `safe_dump` (used by the reference implementation) emits for
    /// list values such as `tags`. Returns [`Value::Null`] when no block
    /// follows.
    fn parse_nested(&mut self, parent_indent: usize) -> Result<Value, YamlError> {
        self.skip_blank_and_comments();
        if self.pos >= self.lines.len() {
            return Ok(Value::Null);
        }
        let ind = self.current_indent()?;
        if ind > parent_indent {
            self.parse_node(ind)
        } else if ind == parent_indent && self.line_is_sequence_item(ind) {
            self.parse_sequence(ind)
        } else {
            Ok(Value::Null)
        }
    }

    /// Whether the current line, taken from column `indent`, begins a block
    /// sequence item (`-` alone or `- …`).
    fn line_is_sequence_item(&self, indent: usize) -> bool {
        let line = &self.lines[self.pos];
        let content = &line[indent.min(line.len())..];
        content == "-" || content.starts_with("- ")
    }

    /// Parses a `|` (literal) or `>` (folded) block scalar. The header (`r`)
    /// is the text after the `key:` (e.g. `|`, `|-`, `>+`).
    fn parse_block_scalar(
        &mut self,
        parent_indent: usize,
        header: &str,
    ) -> Result<Value, YamlError> {
        let style = header.as_bytes()[0]; // b'|' or b'>'
        let chomp = header[1..].chars().find(|c| *c == '+' || *c == '-');

        // Collect body lines: blanks, or lines indented deeper than the parent.
        let mut body: Vec<String> = Vec::new();
        let mut block_indent: Option<usize> = None;
        while self.pos < self.lines.len() {
            let line = &self.lines[self.pos];
            if line.trim().is_empty() {
                body.push(String::new());
                self.pos += 1;
                continue;
            }
            let ind = indent_of(line).ok_or_else(|| self.err("tab in block scalar indentation"))?;
            if ind <= parent_indent {
                break;
            }
            if block_indent.is_none() {
                block_indent = Some(ind);
            }
            let bi = block_indent.unwrap();
            let stripped = if line.len() >= bi {
                line[bi..].to_string()
            } else {
                String::new()
            };
            body.push(stripped);
            self.pos += 1;
        }

        // Drop trailing blank lines for accounting, remember how many there were.
        let mut trailing_blanks = 0;
        while body.last().is_some_and(std::string::String::is_empty) {
            body.pop();
            trailing_blanks += 1;
        }

        let mut text = if style == b'|' {
            body.join("\n")
        } else {
            fold_lines(&body)
        };

        match chomp {
            Some('-') => {} // strip: no trailing newline
            Some('+') => {
                // keep: restore all trailing blank lines + one newline for content
                text.push('\n');
                for _ in 0..trailing_blanks {
                    text.push('\n');
                }
            }
            _ => {
                // clip: exactly one trailing newline if there was any content
                if !text.is_empty() || trailing_blanks > 0 {
                    text.push('\n');
                }
            }
        }
        Ok(Value::String(text))
    }
}

/// Folds a literal block's lines per YAML's folded (`>`) rules: runs of
/// non-empty lines join with a single space; blank lines become newlines.
fn fold_lines(lines: &[String]) -> String {
    let mut out = String::new();
    let mut prev_nonempty = false;
    for line in lines {
        if line.is_empty() {
            out.push('\n');
            prev_nonempty = false;
        } else {
            if prev_nonempty {
                out.push(' ');
            }
            out.push_str(line);
            prev_nonempty = true;
        }
    }
    out
}

/// Whether a (trimmed) line opens a node of its own rather than continuing the
/// scalar above it: a comment, a sequence item, or a mapping entry.
fn starts_a_new_node(content: &str) -> bool {
    content.starts_with('#')
        || content == "-"
        || content.starts_with("- ")
        || split_key_value(content).is_some()
}

/// The quote character of a quoted scalar that `s` opens but does not close, or
/// `None` when `s` is plain or self-contained.
fn unclosed_quote(s: &str) -> Option<char> {
    match s.chars().next()? {
        q @ ('\'' | '"') if !closes_quote(s, q, 1) => Some(q),
        _ => None,
    }
}

/// Whether the closing `quote` of an already-open quoted scalar appears in `s`
/// at or after character index `from`, honouring `''` and `\"` escapes.
fn closes_quote(s: &str, quote: char, from: usize) -> bool {
    let chars: Vec<char> = s.chars().collect();
    let mut i = from;
    while i < chars.len() {
        let c = chars[i];
        if quote == '"' && c == '\\' {
            i += 2;
            continue;
        }
        if c == quote {
            if quote == '\'' && chars.get(i + 1) == Some(&'\'') {
                i += 2;
                continue;
            }
            return true;
        }
        i += 1;
    }
    false
}

/// Leading-space count, or `None` if the indentation contains a tab.
fn indent_of(line: &str) -> Option<usize> {
    let mut n = 0;
    for c in line.chars() {
        match c {
            ' ' => n += 1,
            '\t' => return None,
            _ => break,
        }
    }
    Some(n)
}

/// Splits a (left-trimmed) line into a `key` and optional rest at the first
/// top-level `:` that is followed by a space or end-of-line. Returns `None`
/// when the line is not a mapping entry.
fn split_key_value(s: &str) -> Option<(String, Option<String>)> {
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    let mut quote: Option<char> = None;
    let mut depth: i32 = 0;
    while i < chars.len() {
        let c = chars[i];
        if let Some(q) = quote {
            if q == '"' && c == '\\' {
                i += 2;
                continue;
            }
            if c == q {
                if q == '\'' && chars.get(i + 1) == Some(&'\'') {
                    i += 2;
                    continue;
                }
                quote = None;
            }
            i += 1;
            continue;
        }
        match c {
            '\'' | '"' => quote = Some(c),
            '[' | '{' => depth += 1,
            ']' | '}' => {
                if depth > 0 {
                    depth -= 1;
                }
            }
            '#' if depth == 0 && i > 0 && (chars[i - 1] == ' ' || chars[i - 1] == '\t') => {
                break; // comment region without a preceding separator
            }
            ':' if depth == 0 => {
                let next = chars.get(i + 1).copied();
                if next.is_none() || next == Some(' ') || next == Some('\t') {
                    let key: String = chars[..i].iter().collect();
                    let rest: String = chars[i + 1..].iter().collect();
                    let rest = rest.trim();
                    let rest_opt = if rest.is_empty() || rest.starts_with('#') {
                        None
                    } else {
                        Some(rest.to_string())
                    };
                    return Some((key.trim().to_string(), rest_opt));
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// Parses a single-line value: a flow collection or a scalar.
fn parse_inline_value(s: &str, line: usize) -> Result<Value, YamlError> {
    let t = s.trim();
    if t.starts_with('[') || t.starts_with('{') {
        let mut fp = FlowParser {
            chars: t.chars().collect(),
            pos: 0,
            line,
        };
        let v = fp.parse_value()?;
        fp.skip_ws();
        // Allow a trailing comment after the flow collection.
        if fp.pos < fp.chars.len() && fp.chars[fp.pos] != '#' {
            return Err(YamlError {
                line: line + 1,
                message: "unexpected content after flow collection".into(),
            });
        }
        Ok(v)
    } else {
        parse_scalar(t, line)
    }
}

/// Interprets a scalar token (possibly quoted) into a typed [`Value`].
fn parse_scalar(token: &str, line: usize) -> Result<Value, YamlError> {
    let t = token.trim();
    if t.is_empty() {
        return Ok(Value::Null);
    }
    if t.starts_with('"') {
        return parse_double_quoted(t, line).map(Value::String);
    }
    if t.starts_with('\'') {
        return parse_single_quoted(t, line).map(Value::String);
    }
    // Plain scalar: strip a trailing " #" comment.
    let plain = strip_trailing_comment(t);
    Ok(interpret_plain(plain))
}

/// Strips a trailing ` #...` comment from a plain scalar.
fn strip_trailing_comment(s: &str) -> &str {
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'#' && i > 0 && (bytes[i - 1] == b' ' || bytes[i - 1] == b'\t') {
            return s[..i].trim_end();
        }
        i += 1;
    }
    s.trim_end()
}

/// Resolves a plain (unquoted) scalar to null/bool/int/float/string.
///
/// Number resolution is intentionally conservative to avoid silently coercing
/// identifier-like values: integers must have no redundant leading zero (so a
/// zero-padded code such as `007` stays a string), and floats must contain a
/// decimal point (so `1e3` stays a string). This matches the safe, predictable
/// end of YAML scalar resolution rather than `PyYAML`'s legacy octal/sexagesimal
/// quirks. The special float tokens `.inf`, `-.inf`, and `.nan` are recognized
/// so non-finite floats produced by the emitter round-trip.
fn interpret_plain(s: &str) -> Value {
    match s {
        "" | "~" | "null" | "Null" | "NULL" => return Value::Null,
        "true" | "True" | "TRUE" => return Value::Bool(true),
        "false" | "False" | "FALSE" => return Value::Bool(false),
        ".inf" | ".Inf" | ".INF" | "+.inf" => return Value::Float(f64::INFINITY),
        "-.inf" | "-.Inf" | "-.INF" => return Value::Float(f64::NEG_INFINITY),
        ".nan" | ".NaN" | ".NAN" => return Value::Float(f64::NAN),
        _ => {}
    }
    if is_canonical_int(s)
        && let Ok(i) = s.parse::<i64>()
    {
        return Value::Int(i);
    }
    if is_canonical_float(s)
        && let Ok(f) = s.parse::<f64>()
    {
        return Value::Float(f);
    }
    Value::String(s.to_string())
}

/// `[-+]?(0|[1-9][0-9]*)`: a decimal integer with no redundant leading zero.
fn is_canonical_int(s: &str) -> bool {
    let digits = s.strip_prefix(['+', '-']).unwrap_or(s);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    digits == "0" || !digits.starts_with('0')
}

/// A float that contains a decimal point and a digit (optionally with an
/// exponent), e.g. `0.1`, `-3.5`, `1.0e9`. Bare-exponent forms like `1e3` are
/// deliberately treated as strings.
fn is_canonical_float(s: &str) -> bool {
    if !s.contains('.') || !s.bytes().any(|b| b.is_ascii_digit()) {
        return false;
    }
    s.parse::<f64>().is_ok()
}

fn parse_double_quoted(s: &str, line: usize) -> Result<String, YamlError> {
    let chars: Vec<char> = s.chars().collect();
    debug_assert_eq!(chars[0], '"');
    let mut out = String::new();
    let mut i = 1;
    while i < chars.len() {
        let c = chars[i];
        if c == '"' {
            validate_quoted_tail(&chars, i, line, "double-quoted")?;
            return Ok(out);
        }
        if c == '\\' {
            i += 1;
            let e = *chars.get(i).ok_or_else(|| YamlError {
                line: line + 1,
                message: "dangling escape in double-quoted string".into(),
            })?;
            match e {
                'n' => out.push('\n'),
                't' => out.push('\t'),
                'r' => out.push('\r'),
                '"' => out.push('"'),
                '\\' => out.push('\\'),
                '/' => out.push('/'),
                '0' => out.push('\0'),
                'b' => out.push('\u{0008}'),
                'f' => out.push('\u{000C}'),
                'u' => {
                    let start = i + 1;
                    let end = start + 4;
                    let hex = chars.get(start..end).ok_or_else(|| YamlError {
                        line: line + 1,
                        message: "truncated Unicode escape in double-quoted string".into(),
                    })?;
                    let hex: String = hex.iter().collect();
                    let cp = u32::from_str_radix(&hex, 16).map_err(|_| YamlError {
                        line: line + 1,
                        message: "malformed Unicode escape in double-quoted string".into(),
                    })?;
                    let ch = char::from_u32(cp).ok_or_else(|| YamlError {
                        line: line + 1,
                        message: "invalid Unicode scalar value in double-quoted string".into(),
                    })?;
                    out.push(ch);
                    i = end - 1;
                }
                other => {
                    return Err(YamlError {
                        line: line + 1,
                        message: format!(
                            "unknown escape sequence \\{other} in double-quoted string"
                        ),
                    });
                }
            }
            i += 1;
            continue;
        }
        out.push(c);
        i += 1;
    }
    Err(YamlError {
        line: line + 1,
        message: "unterminated double-quoted string".into(),
    })
}

fn parse_single_quoted(s: &str, line: usize) -> Result<String, YamlError> {
    let chars: Vec<char> = s.chars().collect();
    debug_assert_eq!(chars[0], '\'');
    let mut out = String::new();
    let mut i = 1;
    while i < chars.len() {
        let c = chars[i];
        if c == '\'' {
            if chars.get(i + 1) == Some(&'\'') {
                out.push('\'');
                i += 2;
                continue;
            }
            validate_quoted_tail(&chars, i, line, "single-quoted")?;
            return Ok(out);
        }
        out.push(c);
        i += 1;
    }
    Err(YamlError {
        line: line + 1,
        message: "unterminated single-quoted string".into(),
    })
}

/// Ensures that only whitespace or a trailing comment follows a quoted scalar.
fn validate_quoted_tail(
    chars: &[char],
    close: usize,
    line: usize,
    quote_name: &str,
) -> Result<(), YamlError> {
    for &c in &chars[close + 1..] {
        if c.is_whitespace() {
            continue;
        }
        if c == '#' {
            return Ok(());
        }
        return Err(YamlError {
            line: line + 1,
            message: format!("unexpected content after closing {quote_name} quote"),
        });
    }
    Ok(())
}

/// A recursive parser for flow collections (`[...]`, `{...}`).
struct FlowParser {
    chars: Vec<char>,
    pos: usize,
    line: usize,
}

impl FlowParser {
    fn skip_ws(&mut self) {
        while self.pos < self.chars.len() && self.chars[self.pos].is_whitespace() {
            self.pos += 1;
        }
    }

    fn err(&self, msg: impl Into<String>) -> YamlError {
        YamlError {
            line: self.line + 1,
            message: msg.into(),
        }
    }

    fn parse_value(&mut self) -> Result<Value, YamlError> {
        self.skip_ws();
        match self.chars.get(self.pos) {
            Some('[') => self.parse_seq(),
            Some('{') => self.parse_map(),
            Some(_) => self.parse_flow_scalar(),
            None => Ok(Value::Null),
        }
    }

    fn parse_seq(&mut self) -> Result<Value, YamlError> {
        self.pos += 1; // consume '['
        let mut seq = Vec::new();
        loop {
            self.skip_ws();
            match self.chars.get(self.pos) {
                Some(']') => {
                    self.pos += 1;
                    break;
                }
                None => return Err(self.err("unterminated flow sequence")),
                _ => {}
            }
            seq.push(self.parse_value()?);
            self.skip_ws();
            match self.chars.get(self.pos) {
                Some(',') => self.pos += 1,
                Some(']') => {
                    self.pos += 1;
                    break;
                }
                _ => return Err(self.err("expected ',' or ']' in flow sequence")),
            }
        }
        Ok(Value::Sequence(seq))
    }

    fn parse_map(&mut self) -> Result<Value, YamlError> {
        self.pos += 1; // consume '{'
        let mut map = Mapping::new();
        loop {
            self.skip_ws();
            match self.chars.get(self.pos) {
                Some('}') => {
                    self.pos += 1;
                    break;
                }
                None => return Err(self.err("unterminated flow mapping")),
                _ => {}
            }
            let key = self.parse_flow_scalar()?;
            self.skip_ws();
            // A key with no `: value` is a null-valued entry, as in YAML.
            let value = if self.chars.get(self.pos) == Some(&':') {
                self.pos += 1;
                self.parse_value()?
            } else {
                Value::Null
            };
            map.push_raw(key, value);
            self.skip_ws();
            match self.chars.get(self.pos) {
                Some(',') => self.pos += 1,
                Some('}') => {
                    self.pos += 1;
                    break;
                }
                _ => return Err(self.err("expected ',' or '}' in flow mapping")),
            }
        }
        Ok(Value::Mapping(map))
    }

    fn parse_flow_scalar(&mut self) -> Result<Value, YamlError> {
        self.skip_ws();
        let c = *self
            .chars
            .get(self.pos)
            .ok_or_else(|| self.err("expected scalar"))?;
        if c == '"' || c == '\'' {
            let start = self.pos;
            self.pos += 1;
            while self.pos < self.chars.len() {
                let cur = self.chars[self.pos];
                if c == '"' && cur == '\\' {
                    if self.pos + 1 >= self.chars.len() {
                        return Err(self.err("dangling escape in double-quoted string"));
                    }
                    self.pos += 2;
                    continue;
                }
                if cur == c {
                    if c == '\'' && self.chars.get(self.pos + 1) == Some(&'\'') {
                        self.pos += 2;
                        continue;
                    }
                    self.pos += 1;
                    break;
                }
                self.pos += 1;
            }
            let raw: String = self.chars[start..self.pos].iter().collect();
            let s = if c == '"' {
                parse_double_quoted(&raw, self.line)?
            } else {
                parse_single_quoted(&raw, self.line)?
            };
            return Ok(Value::String(s));
        }
        // Plain flow scalar: read until `,`, `]`, `}`, or a `:` acting as a
        // key/value separator.
        let start = self.pos;
        while self.pos < self.chars.len() {
            match self.chars[self.pos] {
                ',' | ']' | '}' => break,
                ':' if is_separator_colon(&self.chars, self.pos) => break,
                _ => self.pos += 1,
            }
        }
        let raw: String = self.chars[start..self.pos].iter().collect();
        Ok(interpret_plain(raw.trim()))
    }
}

/// Whether the `:` at `i` separates a flow mapping's key from its value, rather
/// than being an ordinary character inside a plain scalar.
///
/// YAML only treats `:` as a separator in flow context when it is followed by
/// whitespace, a flow indicator, or the end of the collection. OKF v0.2 relies
/// on this: `{ by: human:ahormati, at: 2026-06-25T09:00:00Z }` is one mapping
/// of two entries, not a parse error: the colons in `human:ahormati` and
/// `09:00:00` are content.
fn is_separator_colon(chars: &[char], i: usize) -> bool {
    chars
        .get(i + 1)
        .is_none_or(|c| c.is_whitespace() || matches!(c, ',' | '[' | ']' | '{' | '}'))
}
