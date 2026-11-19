//! Parsing, building, and updating `log.md` update histories.
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
//!
//! A `log.md` has no frontmatter. [`Log::parse`] still reads past a block if
//! one is there, so that the entries of an older file are not lost, and
//! [`Log::without_frontmatter`] drops it.

use crate::date::Date;
use crate::document::Document;
use crate::frontmatter::Frontmatter;
use std::fmt::Write as _;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// A parsed `log.md`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Log {
    /// A frontmatter block found at the top of the file. A conforming
    /// `log.md` has none, so this is normally empty; it is kept so that
    /// rewriting an older log does not silently discard what was in it.
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

    /// `true` when the file opened with a frontmatter block, which a
    /// conforming `log.md` does not have.
    #[must_use]
    pub const fn has_frontmatter(&self) -> bool {
        !self.frontmatter.is_empty()
    }

    /// The same log with any frontmatter block dropped.
    #[must_use]
    pub fn without_frontmatter(mut self) -> Self {
        self.frontmatter = Frontmatter::new();
        self
    }

    /// Renders the log back to markdown.
    #[must_use]
    pub fn to_markdown(&self) -> String {
        let mut out = String::new();
        if !self.frontmatter.is_empty() {
            out.push_str("---\n");
            out.push_str(&self.frontmatter.to_yaml_preserving());
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

    /// Appends a new entry under the given date heading.
    ///
    /// If the top (most recent) date heading matches `date`, the entry is appended
    /// to it. Otherwise, a new date section is inserted at the top of the log.
    pub fn append_entry(&mut self, date: &str, kind: Option<&str>, text: &str) {
        let new_entry = LogEntry {
            kind: kind.map(ToString::to_string),
            text: text.to_string(),
        };

        if let Some(first_day) = self.days.first_mut()
            && first_day.date == date
        {
            first_day.entries.push(new_entry);
        } else {
            self.days.insert(
                0,
                LogDay {
                    date: date.to_string(),
                    entries: vec![new_entry],
                },
            );
        }
    }

    /// Returns the date headings that are not valid ISO-8601 `YYYY-MM-DD`
    /// (the spec requires this form).
    #[must_use]
    pub fn invalid_dates(&self) -> Vec<&str> {
        self.days
            .iter()
            .map(|d| d.date.as_str())
            .filter(|d| !is_iso_date(d))
            .collect()
    }

    /// Returns the date groups that are out of order, as
    /// `(heading, the heading it follows)` pairs.
    ///
    /// A log lists the newest date first. A group that is newer than the one
    /// before it breaks that order; the entries are still all readable, so
    /// this is guidance for the producer rather than a conformance failure.
    #[must_use]
    pub fn misordered_dates(&self) -> Vec<(&str, &str)> {
        let mut out = Vec::new();
        let mut previous: Option<(&str, crate::date::Date)> = None;
        for day in &self.days {
            let Some(date) = crate::date::Date::parse(&day.date) else {
                continue;
            };
            if let Some((previous_text, previous_date)) = previous
                && date > previous_date
            {
                out.push((day.date.as_str(), previous_text));
            }
            previous = Some((day.date.as_str(), date));
        }
        out
    }

    /// Returns structural log violations found in the source text.
    ///
    /// [`Log::parse`] intentionally remains a forgiving reader for consumers
    /// that want to recover entries from imperfect Markdown. Conformance
    /// validation uses this stricter pass to ensure that ignored content is
    /// not mistaken for a valid log.
    ///
    /// The order of the date groups is reported separately, by
    /// [`Log::misordered_dates`].
    #[must_use]
    pub fn structural_errors(&self, text: &str) -> Vec<String> {
        let mut errors = Vec::new();

        if self.days.is_empty() {
            errors.push("log contains no date groups".to_string());
        }
        for day in &self.days {
            if day.entries.is_empty() {
                errors.push(format!("log date group {:?} has no entries", day.date));
            }
        }

        let text = text.strip_prefix('\u{feff}').unwrap_or(text);
        let lines: Vec<&str> = text.lines().collect();
        let mut start_idx = 0;
        if !lines.is_empty() && crate::document::is_delimiter(lines[0]) {
            let mut end_idx = None;
            for (i, line) in lines.iter().enumerate().skip(1) {
                if crate::document::is_delimiter(line) {
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

/// Appends an entry to `log.md` in the bundle root, creating the file if needed.
///
/// The entry goes under today's date heading when that is the newest group,
/// and otherwise under a new heading at the top, keeping the newest date
/// first. An existing log is edited in place: every line that was there stays
/// exactly as it was, and the new entry follows the bullet style the log
/// already uses.
///
/// # Errors
///
/// Returns an [`io::Error`] if reading or writing `log.md` fails.
pub fn append_log_entry(
    bundle_root: &Path,
    date: Date,
    kind: &str,
    text: &str,
) -> io::Result<PathBuf> {
    let log_path = bundle_root.join("log.md");
    let content = if log_path.exists() {
        insert_log_entry(
            &fs::read_to_string(&log_path)?,
            &date.to_string(),
            kind,
            text,
        )
    } else {
        format!("# Update Log\n\n## {date}\n* **{kind}**: {text}\n")
    };
    fs::write(&log_path, content)?;
    Ok(log_path)
}

/// Merges date groups that repeat a heading into the first group with that
/// heading, returning the new text and the dates that were merged. `None`
/// when no heading repeats.
///
/// The lines themselves are moved, not re-rendered: wrapped entries, nested
/// bullets, and prose under a heading all arrive as they were written.
pub(crate) fn consolidate_duplicate_dates(text: &str) -> Option<(String, Vec<String>)> {
    let newline = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let lines: Vec<&str> = text.lines().collect();
    let is_heading = |line: &str| line.trim_start().starts_with("## ");

    let first_heading = lines.iter().position(|line| is_heading(line))?;
    // Each group: its heading line, its date, and the lines under it.
    let mut groups: Vec<(&str, String, Vec<&str>)> = Vec::new();
    for line in &lines[first_heading..] {
        if is_heading(line) {
            let date = line.trim_start()[3..].trim().to_string();
            groups.push((line, date, Vec::new()));
        } else if let Some(group) = groups.last_mut() {
            group.2.push(line);
        }
    }

    let mut merged: Vec<(&str, String, Vec<&str>)> = Vec::new();
    let mut repeated: Vec<String> = Vec::new();
    for (heading, date, content) in groups {
        let Some(first) = merged.iter_mut().find(|(_, d, _)| *d == date) else {
            merged.push((heading, date, content));
            continue;
        };
        // Keep the blank lines that closed the first group at its end, and
        // put the repeated group's entries just above them.
        let trailing = first
            .2
            .iter()
            .rev()
            .take_while(|line| line.trim().is_empty())
            .count();
        let keep = first.2.len() - trailing;
        let closing: Vec<&str> = first.2.split_off(keep);
        let start = content
            .iter()
            .position(|line| !line.trim().is_empty())
            .unwrap_or(content.len());
        let end = content
            .iter()
            .rposition(|line| !line.trim().is_empty())
            .map_or(start, |last| last + 1);
        first.2.extend_from_slice(&content[start..end]);
        first.2.extend(closing);
        if !repeated.contains(&date) {
            repeated.push(date);
        }
    }
    if repeated.is_empty() {
        return None;
    }

    let mut out: Vec<&str> = lines[..first_heading].to_vec();
    for (heading, _, content) in &merged {
        out.push(heading);
        out.extend(content.iter());
    }
    // The file ends with its last entry, not with a group separator.
    while out.last().is_some_and(|line| line.trim().is_empty()) {
        out.pop();
    }
    let mut result = out.join(newline);
    result.push_str(newline);
    Some((result, repeated))
}

/// Inserts one entry into the text of a log, touching no other line.
fn insert_log_entry(content: &str, date: &str, kind: &str, text: &str) -> String {
    let newline = if content.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let mut lines: Vec<String> = content.lines().map(ToString::to_string).collect();
    let is_date_heading = |line: &str| line.trim_start().starts_with("## ");

    // Follow the conventions the log already has: its bullet marker, and
    // whether a blank line separates a date heading from its entries.
    let marker = lines
        .iter()
        .map(|line| line.trim_start())
        .find_map(|line| bullet_body(line).map(|_| &line[..1]))
        .unwrap_or("*")
        .to_string();
    let entry = format!("{marker} **{kind}**: {text}");
    let first_heading = lines.iter().position(|line| is_date_heading(line));
    let spaced = first_heading
        .and_then(|i| lines.get(i + 1))
        .is_some_and(|line| line.trim().is_empty());

    match first_heading {
        // Today's group is the newest one: add to the end of it.
        Some(i) if lines[i].trim_start()[3..].trim() == date => {
            let group_end = lines[i + 1..]
                .iter()
                .position(|line| is_date_heading(line))
                .map_or(lines.len(), |offset| i + 1 + offset);
            let last_content = (i + 1..group_end)
                .rev()
                .find(|&j| !lines[j].trim().is_empty())
                .unwrap_or(i);
            // An empty group keeps the blank line under its heading.
            let at = if last_content == i && spaced {
                i + 2
            } else {
                last_content + 1
            };
            lines.insert(at.min(lines.len()), entry);
        }
        // A newer date: a new group goes above the newest existing one.
        Some(i) => {
            let mut group = vec![format!("## {date}")];
            if spaced {
                group.push(String::new());
            }
            group.push(entry);
            group.push(String::new());
            lines.splice(i..i, group);
        }
        // No groups yet: start the first one at the end of the file.
        None => {
            if lines.last().is_some_and(|line| !line.trim().is_empty()) {
                lines.push(String::new());
            }
            lines.push(format!("## {date}"));
            lines.push(entry);
        }
    }

    let mut out = lines.join(newline);
    out.push_str(newline);
    out
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appending_touches_no_existing_line() {
        // A log in the style the OKF example bundles use: `-` bullets, a
        // blank line under each heading, no colon after the bold word.
        let log = "# Bundle history\n\n## 2026-10-28\n\n- **Updated** the legacy metric.\n\n## 2026-10-23\n\n- **Migrated** the bundle.\n";

        // A newer date opens a new group above, in the same style.
        let newer = insert_log_entry(log, "2026-11-19", "Update", "Renamed a concept.");
        assert_eq!(
            newer,
            "# Bundle history\n\n## 2026-11-19\n\n- **Update**: Renamed a concept.\n\n## 2026-10-28\n\n- **Updated** the legacy metric.\n\n## 2026-10-23\n\n- **Migrated** the bundle.\n"
        );

        // The same date as the newest group adds to the end of that group.
        let same = insert_log_entry(log, "2026-10-28", "Update", "Renamed a concept.");
        assert_eq!(
            same,
            "# Bundle history\n\n## 2026-10-28\n\n- **Updated** the legacy metric.\n- **Update**: Renamed a concept.\n\n## 2026-10-23\n\n- **Migrated** the bundle.\n"
        );

        // Both results still read as logs, newest first.
        for text in [&newer, &same] {
            let parsed = Log::parse(text);
            assert!(parsed.structural_errors(text).is_empty(), "{text}");
            assert!(parsed.misordered_dates().is_empty(), "{text}");
        }
    }

    #[test]
    fn appending_starts_a_log_that_has_no_groups() {
        assert_eq!(
            insert_log_entry("# Update Log\n", "2026-11-19", "Creation", "Made."),
            "# Update Log\n\n## 2026-11-19\n* **Creation**: Made.\n"
        );
        // The compact style this crate writes is kept as it is.
        let compact = "# Update Log\n\n## 2026-11-19\n* **Creation**: Made.\n";
        assert_eq!(
            insert_log_entry(compact, "2026-11-19", "Update", "Moved."),
            "# Update Log\n\n## 2026-11-19\n* **Creation**: Made.\n* **Update**: Moved.\n"
        );
        assert_eq!(
            insert_log_entry(compact, "2026-11-20", "Update", "Moved."),
            "# Update Log\n\n## 2026-11-20\n* **Update**: Moved.\n\n## 2026-11-19\n* **Creation**: Made.\n"
        );
    }

    #[test]
    fn a_log_with_frontmatter_is_read_and_can_drop_it() {
        let text = "---\ntype: Log\n---\n\n# History\n\n## 2026-06-30\n* **Creation**: Made.\n";
        let log = Log::parse(text);
        assert!(log.has_frontmatter());
        assert_eq!(log.days.len(), 1);
        let stripped = log.without_frontmatter();
        assert!(!stripped.has_frontmatter());
        assert!(stripped.to_markdown().starts_with("# History\n"));
    }

    #[test]
    fn misordered_dates_are_reported_separately_from_structure() {
        let text = "# Log\n\n## 2026-06-01\n* one\n\n## 2026-06-02\n* two\n";
        let log = Log::parse(text);
        assert_eq!(log.structural_errors(text), Vec::<String>::new());
        assert_eq!(log.misordered_dates(), vec![("2026-06-02", "2026-06-01")]);
    }
}
