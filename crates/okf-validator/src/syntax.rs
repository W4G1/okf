//! Multi-language syntax validation for Attested Computations and embedded scripts.
//!
//! Provides fast syntax checking for Python,
//! JavaScript, TypeScript, Rust, SQL, JSON, YAML, and Bash using pure Rust AST parsers.

use std::fmt;

/// Supported languages for static syntax checking.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    /// Python script (`.py`, `python`, `python3`)
    Python,
    /// JavaScript script (`.js`, `.mjs`, `.cjs`, `javascript`, `node`)
    JavaScript,
    /// TypeScript script (`.ts`, `.mts`, `.cts`, `.tsx`, `.jsx`, `typescript`, `deno`, `bun`)
    TypeScript,
    /// Rust source (`.rs`, `rust`)
    Rust,
    /// SQL query / migration (`.sql`, `sql`)
    Sql,
    /// JSON data structure (`.json`, `json`)
    Json,
    /// YAML document (`.yaml`, `.yml`, `yaml`)
    Yaml,
    /// Bash / POSIX shell script (`.sh`, `.bash`, `shell`, `sh`)
    Bash,
    /// Unknown or unsupported language tag.
    Unknown,
}

impl Language {
    /// Identifies the language from a language tag, runtime identifier, or file extension.
    #[must_use]
    pub fn from_tag(tag: &str) -> Self {
        let tag = tag.trim().to_ascii_lowercase();
        // Strip common suffixes like `rust,no_run` or `python,ignore`
        let tag = tag.split([',', ' ']).next().unwrap_or(&tag);
        match tag {
            "py" | "python" | "python3" => Self::Python,
            "js" | "javascript" | "node" | "nodejs" | "mjs" | "cjs" => Self::JavaScript,
            "ts" | "typescript" | "deno" | "bun" | "mts" | "cts" | "tsx" | "jsx" => {
                Self::TypeScript
            }
            "rs" | "rust" => Self::Rust,
            "sql" => Self::Sql,
            "json" => Self::Json,
            "yaml" | "yml" => Self::Yaml,
            "sh" | "bash" | "zsh" | "shell" => Self::Bash,
            _ => Self::Unknown,
        }
    }

    /// Display name of the language.
    #[must_use]
    pub const fn name(&self) -> &'static str {
        self.as_str()
    }

    /// Returns the canonical name of the language as a string slice.
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Python => "python",
            Self::JavaScript => "javascript",
            Self::TypeScript => "typescript",
            Self::Rust => "rust",
            Self::Sql => "sql",
            Self::Json => "json",
            Self::Yaml => "yaml",
            Self::Bash => "bash",
            Self::Unknown => "unknown",
        }
    }
}

impl fmt::Display for Language {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl AsRef<str> for Language {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl std::str::FromStr for Language {
    type Err = std::convert::Infallible;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self::from_tag(s))
    }
}

/// A parsed fenced code block from markdown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FencedCodeBlock {
    /// The language tag if specified (e.g. `python`, `rust`).
    pub language: Option<String>,
    /// The inner code content.
    pub code: String,
    /// 1-based start line of the opening fence in the markdown document.
    pub start_line: usize,
}

/// Extracts all fenced code blocks (delimited by 3+ backticks or 3+ tildes) from markdown text.
#[must_use]
pub fn extract_fenced_code_blocks(markdown: &str) -> Vec<FencedCodeBlock> {
    let mut blocks = Vec::new();
    let mut in_fence = false;
    let mut fence_char = '`';
    let mut fence_len = 0;
    let mut lang: Option<String> = None;
    let mut block_lines: Vec<&str> = Vec::new();
    let mut start_line = 0;

    for (line_idx, line) in markdown.lines().enumerate() {
        let line_no = line_idx + 1;
        let trimmed = line.trim_start();
        let indent = line.len() - trimmed.len();

        if in_fence {
            let close_indent = line.len() - trimmed.len();
            let is_close = close_indent <= 3 && {
                let count = trimmed.chars().take_while(|&c| c == fence_char).count();
                count >= fence_len && trimmed[count..].trim().is_empty()
            };

            if is_close {
                in_fence = false;
                let code = block_lines.join("\n");
                blocks.push(FencedCodeBlock {
                    language: lang.take(),
                    code,
                    start_line,
                });
            } else {
                block_lines.push(line);
            }
        } else if indent <= 3 && (trimmed.starts_with("```") || trimmed.starts_with("~~~")) {
            let ch = trimmed.chars().next().unwrap_or('`');
            let count = trimmed.chars().take_while(|&c| c == ch).count();
            if count >= 3 {
                in_fence = true;
                fence_char = ch;
                fence_len = count;
                start_line = line_no;
                let tag = trimmed[count..].trim();
                let first_tag = tag.split([',', ' ', '\t']).next().unwrap_or("");
                lang = if first_tag.is_empty() {
                    None
                } else {
                    Some(first_tag.to_string())
                };
                block_lines.clear();
            }
        }
    }

    blocks
}

/// A syntax diagnostic produced by static AST parsing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyntaxError {
    /// The language that was being parsed.
    pub language: String,
    /// Human-readable error description.
    pub message: String,
    /// 1-based line number if available.
    pub line: Option<usize>,
    /// 1-based column number if available.
    pub column: Option<usize>,
}

impl fmt::Display for SyntaxError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(line) = self.line {
            if let Some(col) = self.column {
                write!(f, "{line}:{col}: {}", self.message)
            } else {
                write!(f, "{line}: {}", self.message)
            }
        } else {
            write!(f, "{}", self.message)
        }
    }
}

impl std::error::Error for SyntaxError {}

fn offset_to_line_col(source: &str, offset: usize) -> (Option<usize>, Option<usize>) {
    let bounded = offset.min(source.len());
    let safe_offset = source.floor_char_boundary(bounded);
    let line = source[..safe_offset].matches('\n').count() + 1;
    let last_newline = source[..safe_offset].rfind('\n').map_or(0, |idx| idx + 1);
    let column = source[last_newline..safe_offset].chars().count() + 1;
    (Some(line), Some(column))
}

/// Checks the syntax of `source` for the given `language` tag or name.
///
/// Returns `Ok(())` if the syntax is valid or if the language is unknown / unsupported,
/// or `Err(SyntaxError)` if a syntax error is discovered.
///
/// # Errors
///
/// Returns [`SyntaxError`] if the source code contains syntax or parse errors.
pub fn check_syntax(language_tag: &str, source: &str) -> Result<(), SyntaxError> {
    let lang = Language::from_tag(language_tag);
    match lang {
        Language::Python => check_python(source),
        Language::JavaScript => check_javascript(source, false),
        Language::TypeScript => check_javascript(source, true),
        Language::Rust => check_rust(source),
        Language::Sql => check_sql(source),
        Language::Json => check_json(source),
        Language::Yaml => check_yaml(source),
        Language::Bash => check_bash(source),
        Language::Unknown => Ok(()),
    }
}

fn check_python(source: &str) -> Result<(), SyntaxError> {
    match rustpython_parser::parse(source, rustpython_parser::Mode::Module, "<computation>") {
        Ok(_) => Ok(()),
        Err(err) => {
            let (line, column) = offset_to_line_col(source, err.offset.to_usize());
            Err(SyntaxError {
                language: "python".to_string(),
                message: err.error.to_string(),
                line,
                column,
            })
        }
    }
}

fn check_javascript(source: &str, typescript: bool) -> Result<(), SyntaxError> {
    let allocator = oxc_allocator::Allocator::default();
    let mut source_type = oxc_span::SourceType::default().with_module(true);
    if typescript {
        source_type = source_type.with_typescript(true).with_jsx(true);
    } else {
        source_type = source_type.with_jsx(true);
    }

    let parser = oxc_parser::Parser::new(&allocator, source, source_type);
    let ret = parser.parse();

    if let Some(first_diag) = ret.diagnostics.first() {
        let (line, column) = first_diag.labels.first().map_or((None, None), |l| {
            let offset = usize::try_from(l.offset()).unwrap_or(0);
            offset_to_line_col(source, offset)
        });

        let msg = first_diag.to_string();
        let lang = if typescript {
            "typescript"
        } else {
            "javascript"
        };
        return Err(SyntaxError {
            language: lang.to_string(),
            message: msg,
            line,
            column,
        });
    }

    Ok(())
}

fn check_rust(source: &str) -> Result<(), SyntaxError> {
    if syn::parse_file(source).is_ok() {
        return Ok(());
    }
    // Try wrapping in a function body to allow statements / snippet validation
    let wrapped = format!("fn __okf_snippet_check__() {{\n{source}\n}}");
    if syn::parse_file(&wrapped).is_ok() {
        return Ok(());
    }
    if syn::parse_str::<syn::Item>(source).is_ok() {
        return Ok(());
    }

    match syn::parse_file(source) {
        Ok(_) => Ok(()),
        Err(err) => Err(SyntaxError {
            language: "rust".to_string(),
            message: err.to_string(),
            line: None,
            column: None,
        }),
    }
}

fn check_sql(source: &str) -> Result<(), SyntaxError> {
    let dialect = sqlparser::dialect::GenericDialect {};
    match sqlparser::parser::Parser::parse_sql(&dialect, source) {
        Ok(_) => Ok(()),
        Err(err) => Err(SyntaxError {
            language: "sql".to_string(),
            message: err.to_string(),
            line: None,
            column: None,
        }),
    }
}

fn check_json(source: &str) -> Result<(), SyntaxError> {
    match serde_json::from_str::<serde_json::Value>(source) {
        Ok(_) => Ok(()),
        Err(err) => Err(SyntaxError {
            language: "json".to_string(),
            message: err.to_string(),
            line: Some(err.line()),
            column: Some(err.column()),
        }),
    }
}

fn check_yaml(source: &str) -> Result<(), SyntaxError> {
    match okf_core::yaml::Value::parse(source) {
        Ok(_) => Ok(()),
        Err(err) => Err(SyntaxError {
            language: "yaml".to_string(),
            message: err.to_string(),
            line: None,
            column: None,
        }),
    }
}

fn check_bash(source: &str) -> Result<(), SyntaxError> {
    let mut quote: Option<char> = None;
    let mut escaped = false;
    let mut paren_depth: usize = 0;
    let mut brace_depth: usize = 0;

    for (line_idx, line) in source.lines().enumerate() {
        let line_no = line_idx + 1;
        let mut prev_ch: Option<char> = None;

        for (col_idx, ch) in line.chars().enumerate() {
            let col_no = col_idx + 1;

            if quote.is_none()
                && (prev_ch.is_none()
                    || prev_ch == Some(' ')
                    || prev_ch == Some('\t')
                    || prev_ch == Some(';')
                    || prev_ch == Some('&')
                    || prev_ch == Some('|'))
                && ch == '#'
            {
                break;
            }

            if quote == Some('\'') {
                if ch == '\'' {
                    quote = None;
                }
                prev_ch = Some(ch);
                continue;
            }

            if escaped {
                escaped = false;
                prev_ch = Some(ch);
                continue;
            }

            if ch == '\\' {
                escaped = true;
                prev_ch = Some(ch);
                continue;
            }

            if let Some(q) = quote {
                if ch == q {
                    quote = None;
                }
            } else {
                match ch {
                    '\'' | '"' | '`' => quote = Some(ch),
                    '(' => paren_depth += 1,
                    ')' => {
                        if paren_depth == 0 {
                            return Err(SyntaxError {
                                language: "bash".to_string(),
                                message: "unexpected closing parenthesis ')'".to_string(),
                                line: Some(line_no),
                                column: Some(col_no),
                            });
                        }
                        paren_depth -= 1;
                    }
                    '{' => brace_depth += 1,
                    '}' => {
                        if brace_depth == 0 {
                            return Err(SyntaxError {
                                language: "bash".to_string(),
                                message: "unexpected closing brace '}'".to_string(),
                                line: Some(line_no),
                                column: Some(col_no),
                            });
                        }
                        brace_depth -= 1;
                    }
                    _ => {}
                }
            }
            prev_ch = Some(ch);
        }
    }

    if let Some(q) = quote {
        return Err(SyntaxError {
            language: "bash".to_string(),
            message: format!("unclosed quote `{q}`"),
            line: None,
            column: None,
        });
    }
    if paren_depth > 0 {
        return Err(SyntaxError {
            language: "bash".to_string(),
            message: "unclosed parenthesis '('".to_string(),
            line: None,
            column: None,
        });
    }
    if brace_depth > 0 {
        return Err(SyntaxError {
            language: "bash".to_string(),
            message: "unclosed brace '{'".to_string(),
            line: None,
            column: None,
        });
    }

    Ok(())
}
