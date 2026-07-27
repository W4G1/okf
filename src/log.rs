//! Parsing and building `log.md` update histories (§9).
//!
//! A log is a flat list of date-grouped entries, newest first:
//!
//! ```text
//! # Directory Update Log
//!
//! ## 2026-05-22
//! * **Update**: Added a new table reference.
//! * **Creation**: Established the playbook.
//! ```
//!
//! Date headings use ISO-8601 `YYYY-MM-DD`. The leading bold word
//! (`**Update**`, `**Creation**`, …) is a convention, not a requirement.

use std::fmt::Write as _;

/// A parsed `log.md`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Log {
    /// The top-level `# ` heading text, if any.
    pub title: Option<String>,
    /// Date-grouped entries, in document order (the convention is newest-first).
    pub days: Vec<LogDay>,
}

/// All entries recorded under a single date heading.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogDay {
    /// The `## ` heading text (an ISO-8601 date by convention).
    pub date: String,
    /// The bullet entries under this date.
    pub entries: Vec<LogEntry>,
}

/// A single log bullet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogEntry {
    /// The leading bold marker (`Update`, `Creation`, …), if present.
    pub kind: Option<String>,
    /// The entry prose (everything after the optional marker).
    pub text: String,
}

impl Log {
    /// Parses `log.md` text.
    #[must_use]
    pub fn parse(text: &str) -> Self {
        let mut log = Self::default();
        let mut current: Option<LogDay> = None;

        for line in text.lines() {
            let trimmed = line.trim_end();
            let t = trimmed.trim_start();
            if let Some(rest) = t.strip_prefix("## ") {
                if let Some(day) = current.take() {
                    log.days.push(day);
                }
                current = Some(LogDay {
                    date: rest.trim().to_string(),
                    entries: Vec::new(),
                });
            } else if let Some(rest) = t.strip_prefix("# ") {
                if log.title.is_none() && current.is_none() {
                    log.title = Some(rest.trim().to_string());
                }
            } else if let Some(rest) = bullet_body(t) {
                if let Some(day) = current.as_mut() {
                    day.entries.push(parse_entry(rest));
                }
            }
        }
        if let Some(day) = current.take() {
            log.days.push(day);
        }
        log
    }

    /// Renders the log back to markdown.
    #[must_use]
    pub fn to_markdown(&self) -> String {
        let mut out = String::new();
        if let Some(title) = &self.title {
            let _ = writeln!(out, "# {title}");
            out.push('\n');
        }
        for (i, day) in self.days.iter().enumerate() {
            if i > 0 {
                out.push('\n');
            }
            let _ = writeln!(out, "## {}", day.date);
            for entry in &day.entries {
                match &entry.kind {
                    Some(kind) => {
                        let _ = writeln!(out, "* **{kind}**: {}", entry.text);
                    }
                    None => {
                        let _ = writeln!(out, "* {}", entry.text);
                    }
                }
            }
        }
        out
    }

    /// Returns the date headings that are not valid ISO-8601 `YYYY-MM-DD`
    /// (§9 requires this form).
    #[must_use]
    pub fn invalid_dates(&self) -> Vec<&str> {
        self.days
            .iter()
            .map(|d| d.date.as_str())
            .filter(|d| !is_iso_date(d))
            .collect()
    }
}

/// Returns the text after a `*` or `-` bullet marker, if the line is a bullet.
fn bullet_body(line: &str) -> Option<&str> {
    line.strip_prefix("* ").or_else(|| line.strip_prefix("- "))
}

/// Parses a bullet body into an optional bold `kind` and the remaining text.
fn parse_entry(body: &str) -> LogEntry {
    let b = body.trim();
    if let Some(rest) = b.strip_prefix("**") {
        if let Some(end) = rest.find("**") {
            let kind = rest[..end].trim().to_string();
            let mut text = rest[end + 2..].trim_start();
            text = text.strip_prefix(':').unwrap_or(text).trim_start();
            return LogEntry {
                kind: Some(kind),
                text: text.to_string(),
            };
        }
    }
    LogEntry {
        kind: None,
        text: b.to_string(),
    }
}

/// Checks that a string is a valid ISO-8601 calendar date (`YYYY-MM-DD`).
#[must_use]
pub fn is_iso_date(s: &str) -> bool {
    crate::date::Date::parse(s).is_some()
}
