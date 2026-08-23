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

use crate::document::Document;
use crate::frontmatter::Frontmatter;
use std::fmt::Write as _;

/// A parsed `log.md`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Log {
    /// Optional frontmatter block.
    pub frontmatter: Frontmatter,
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
        let (frontmatter, body) = match Document::parse(text) {
            Ok(doc) => (doc.frontmatter, doc.body),
            Err(_) => (Frontmatter::new(), text.to_string()),
        };
        let mut log = Self {
            frontmatter,
            title: None,
            days: Vec::new(),
        };
        let mut current: Option<LogDay> = None;

        for line in body.lines() {
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
            } else if let Some(rest) = bullet_body(t)
                && let Some(day) = current.as_mut()
            {
                day.entries.push(parse_entry(rest));
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
        if !self.frontmatter.is_empty() {
            let fm_text =
                crate::yaml::Value::Mapping(self.frontmatter.as_mapping().clone()).to_yaml_string();
            out.push_str("---\n");
            out.push_str(&fm_text);
            out.push_str("---\n\n");
        }
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

    /// Returns structural §9 violations found in the source text.
    ///
    /// [`Log::parse`] intentionally remains a forgiving reader for consumers
    /// that want to recover entries from imperfect Markdown. Conformance
    /// validation uses this stricter pass to ensure that ignored content is
    /// not mistaken for a valid log.
    pub(crate) fn structural_errors(&self, text: &str) -> Vec<String> {
        let mut errors = Vec::new();

        if self.days.is_empty() {
            errors.push("log contains no date groups".to_string());
        }
        for day in &self.days {
            if day.entries.is_empty() {
                errors.push(format!("log date group {:?} has no entries", day.date));
            }
        }

        let mut previous: Option<(&str, crate::date::Date)> = None;
        for day in &self.days {
            let Some(date) = crate::date::Date::parse(&day.date) else {
                continue;
            };
            if let Some((previous_text, previous_date)) = previous
                && date > previous_date
            {
                errors.push(format!(
                    "log date groups are not newest first: {:?} follows {:?}",
                    day.date, previous_text
                ));
            }
            previous = Some((day.date.as_str(), date));
        }

        let lines: Vec<&str> = text.lines().collect();
        let mut start_idx = 0;
        if !lines.is_empty() && lines[0].trim() == "---" {
            let mut end_idx = None;
            for (i, line) in lines.iter().enumerate().skip(1) {
                if line.trim() == "---" {
                    end_idx = Some(i);
                    break;
                }
            }
            if let Some(end) = end_idx {
                start_idx = end + 1;
            } else {
                errors.push("Unterminated YAML frontmatter block".to_string());
            }
        }

        let mut saw_date = false;
        let mut saw_title = false;
        let mut current_has_entry = false;
        for (line_offset, line) in lines[start_idx..].iter().enumerate() {
            let line_index = start_idx + line_offset;
            let trimmed = line.trim_end();
            let t = trimmed.trim_start();
            if t.is_empty() {
                continue;
            }
            if t.starts_with("## ") {
                saw_date = true;
                current_has_entry = false;
                continue;
            }
            if t.starts_with("# ") {
                if saw_date || saw_title {
                    errors.push(format!(
                        "log contains non-log content at line {}",
                        line_index + 1
                    ));
                } else {
                    saw_title = true;
                }
                continue;
            }
            if bullet_body(t).is_some() {
                if saw_date {
                    current_has_entry = true;
                } else {
                    errors.push(format!(
                        "log contains an entry outside a date group at line {}",
                        line_index + 1
                    ));
                }
                continue;
            }

            // Markdown permits an entry's prose to continue on an indented
            // line. Unindented prose is not silently assigned to a group.
            if saw_date && current_has_entry && line.chars().next().is_some_and(char::is_whitespace)
            {
                continue;
            }
            errors.push(format!(
                "log contains non-log content at line {}",
                line_index + 1
            ));
        }
        errors
    }
}

/// Returns the text after a `*` or `-` bullet marker, if the line is a bullet.
fn bullet_body(line: &str) -> Option<&str> {
    line.strip_prefix("* ").or_else(|| line.strip_prefix("- "))
}

/// Parses a bullet body into an optional bold `kind` and the remaining text.
fn parse_entry(body: &str) -> LogEntry {
    let b = body.trim();
    if let Some(rest) = b.strip_prefix("**")
        && let Some(end) = rest.find("**")
    {
        let kind = rest[..end].trim().to_string();
        let mut text = rest[end + 2..].trim_start();
        text = text.strip_prefix(':').unwrap_or(text).trim_start();
        return LogEntry {
            kind: Some(kind),
            text: text.to_string(),
        };
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
