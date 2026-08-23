//! The OKF concept document: YAML frontmatter + markdown body.
//!
//! The parse, serialize, and validation behaviour is a faithful port of the
//! reference implementation's `OKFDocument`
//! (`okf/src/reference_agent/bundle/document.py`), so documents round-trip
//! compatibly between the two. Ported to Rust and modified from the original
//! Apache-2.0 Python source; see the NOTICE file.
//!
//! On top of parsing, [`Document`] exposes the v0.2 body conventions that pair
//! with frontmatter: footnote attribution keyed to `sources[].id` (§5.1) and
//! the `# Computation` block of an Attested Computation (§10.3).

use crate::computation::{AttestedComputation, InlineComputation};
use crate::error::DocumentError;
use crate::footnotes::{self, FootnoteDef, FootnoteRef};
use crate::frontmatter::{Frontmatter, RECOMMENDED_FRONTMATTER_KEYS, REQUIRED_FRONTMATTER_KEYS};
use crate::links::{self, Citation, Link};
use crate::provenance::{self, Attribution};
use crate::yaml::Value;

const FRONTMATTER_DELIM: &str = "---";

/// A parsed OKF concept document.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Document {
    /// The YAML frontmatter block (empty if the file had none).
    pub frontmatter: Frontmatter,
    /// Everything after the frontmatter.
    pub body: String,
}

impl Document {
    /// Creates a document from frontmatter and a body.
    pub fn new(frontmatter: Frontmatter, body: impl Into<String>) -> Self {
        Self {
            frontmatter,
            body: body.into(),
        }
    }

    /// Parses a document from raw file text.
    ///
    /// If the file does not begin with a `---` frontmatter delimiter, the
    /// entire text is treated as the body and the frontmatter is empty
    /// (matching the reference parser). An opened-but-unclosed frontmatter
    /// block is an error.
    ///
    /// # Line endings
    ///
    /// The two paths handle line endings the same way the reference
    /// implementation does, by deliberate parity:
    /// - **No frontmatter**: the body is kept verbatim, so a file with CRLF
    ///   line endings round-trips byte-identically. This mirrors the
    ///   reference's `return cls(frontmatter={}, body=text)`.
    /// - **With frontmatter**: the body is rebuilt via `lines().join("\n")`,
    ///   which normalizes `\r\n` (and a trailing `\r`) to `\n`. This mirrors
    ///   the reference's `text.splitlines()` + `"\n".join(...)`. Anything
    ///   inside the frontmatter block is likewise normalized before YAML
    ///   parsing.
    ///
    /// # Errors
    ///
    /// Returns [`DocumentError::UnterminatedFrontmatter`] if the opening `---`
    /// has no matching close, [`DocumentError::InvalidYaml`] if the frontmatter
    /// is not valid YAML, and [`DocumentError::FrontmatterNotMapping`] if it
    /// parses to a scalar or sequence rather than a mapping.
    pub fn parse(text: &str) -> Result<Self, DocumentError> {
        let lines: Vec<&str> = text.lines().collect();
        if lines.is_empty() || lines[0].trim() != FRONTMATTER_DELIM {
            return Ok(Self {
                frontmatter: Frontmatter::new(),
                body: text.to_string(),
            });
        }

        let mut end_idx = None;
        for (i, line) in lines.iter().enumerate().skip(1) {
            if line.trim() == FRONTMATTER_DELIM {
                end_idx = Some(i);
                break;
            }
        }
        let end_idx = end_idx.ok_or(DocumentError::UnterminatedFrontmatter)?;

        let fm_text = lines[1..end_idx].join("\n");
        let value = Value::parse(&fm_text)?;
        let frontmatter = match value {
            Value::Null => Frontmatter::new(),
            Value::Mapping(m) => Frontmatter::from_mapping(m),
            _ => return Err(DocumentError::FrontmatterNotMapping),
        };

        let mut body = lines[end_idx + 1..].join("\n");
        if let Some(stripped) = body.strip_prefix('\n') {
            body = stripped.to_string();
        }

        Ok(Self { frontmatter, body })
    }

    /// Serializes the document back to text: frontmatter delimited by `---`,
    /// a blank line, then the body (terminated by a newline).
    ///
    /// `parse` followed by `serialize` preserves frontmatter key order and the
    /// body (modulo trailing-newline normalization), matching the reference.
    /// Flow collections are re-emitted in block style, which is the same value
    /// written differently.
    #[must_use]
    pub fn serialize(&self) -> String {
        let fm_text = Value::Mapping(self.frontmatter.as_mapping().clone())
            .to_yaml_string()
            .trim_end()
            .to_string();
        let body = if self.body.ends_with('\n') {
            self.body.clone()
        } else {
            format!("{}\n", self.body)
        };
        format!("{FRONTMATTER_DELIM}\n{fm_text}\n{FRONTMATTER_DELIM}\n\n{body}")
    }

    /// Validates the document against §11: the frontmatter must carry a
    /// non-empty `type`, and nothing else is required.
    ///
    /// That single check is the whole of document-level validation in v0.2, and
    /// it matches the reference implementation's `OKFDocument.validate`. Every
    /// other field the spec describes is a SHOULD, so a concept carrying only
    /// `type` passes here; see [`Document::missing_recommended`] for the
    /// producer-side checklist and
    /// [`validate_bundle`](crate::validate_bundle) for the full diagnostics.
    ///
    /// # Errors
    ///
    /// Returns [`DocumentError::MissingKeys`] listing every required key that
    /// is absent, empty, or has the wrong shape.
    pub fn validate(&self) -> Result<(), DocumentError> {
        let missing: Vec<String> = REQUIRED_FRONTMATTER_KEYS
            .iter()
            .filter(|&&key| {
                let Some(value) = self.frontmatter.get(key) else {
                    return true;
                };
                value.is_empty_value() || (key == "type" && value.as_display_str().is_none())
            })
            .map(|key| (*key).to_string())
            .collect();

        if missing.is_empty() {
            Ok(())
        } else {
            Err(DocumentError::MissingKeys(missing))
        }
    }

    /// The [recommended](RECOMMENDED_FRONTMATTER_KEYS) frontmatter keys this
    /// document leaves unset, plus `runtime` when the concept is an Attested
    /// Computation, which §10.2 requires it to carry.
    ///
    /// None of these is a conformance failure, so [`Document::validate`]
    /// ignores them: §11 forbids rejecting a concept for a missing optional
    /// field. This is the checklist a *producer* wants before publishing, and
    /// it is what [`validate_bundle`](crate::validate_bundle) reports as
    /// warnings. An empty result means the document is fully filled in.
    ///
    /// `generated` counts as set when a legacy v0.1 `timestamp` stands in for
    /// it, since §13.1 lets consumers read one for the other.
    #[must_use]
    pub fn missing_recommended(&self) -> Vec<&'static str> {
        let mut missing: Vec<&'static str> = RECOMMENDED_FRONTMATTER_KEYS
            .iter()
            .copied()
            .filter(|key| match *key {
                "generated" => !self.has("generated") && !self.has("timestamp"),
                other => !self.has(other),
            })
            .collect();

        if self.frontmatter.is_attested_computation() && !self.has("runtime") {
            missing.push("runtime");
        }

        missing
    }

    /// Whether a frontmatter key is present and carries a non-empty value.
    fn has(&self, key: &str) -> bool {
        self.frontmatter
            .get(key)
            .is_some_and(|value| !value.is_empty_value())
    }

    /// Extracts all markdown links found in the body (§6.1).
    #[must_use]
    pub fn links(&self) -> Vec<Link> {
        links::extract_links(&self.body)
    }

    /// The non-blank lines under a top-level `# heading` in the body, up to the
    /// next top-level heading.
    ///
    /// §4.2 gives `# Schema`, `# Examples`, and `# Computation` conventional
    /// meaning without attaching required behaviour, so this is the primitive a
    /// consumer needs to read any of them. A port of the reference's
    /// `_section_content_lines`, including its details: `heading` is matched in
    /// full (pass `"# Schema"`), only `# ` counts as a heading so `##`
    /// subheadings stay inside the section, and each line keeps its original
    /// indentation.
    ///
    /// Returns an empty vector when no such section exists. A repeated heading
    /// contributes its lines to the same result.
    #[must_use]
    pub fn section(&self, heading: &str) -> Vec<&str> {
        let mut in_section = false;
        let mut lines = Vec::new();
        for line in self.body.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("# ") {
                in_section = trimmed == heading;
                continue;
            }
            if in_section && !trimmed.is_empty() {
                lines.push(line);
            }
        }
        lines
    }

    /// Extracts the body's `[^label]` attribution markers (§5.1).
    #[must_use]
    pub fn footnote_refs(&self) -> Vec<FootnoteRef> {
        footnotes::extract_refs(&self.body)
    }

    /// Extracts the body's `[^label]: text` footnote definitions (§5.1).
    #[must_use]
    pub fn footnote_definitions(&self) -> Vec<FootnoteDef> {
        footnotes::extract_definitions(&self.body)
    }

    /// Joins the body's footnotes to the `sources` entries they name, giving
    /// per-claim attribution (§5.1).
    ///
    /// Labels that match no source are still returned, with
    /// [`Attribution::source`] set to `None`.
    #[must_use]
    pub fn attributions(&self) -> Vec<Attribution> {
        provenance::attributions(&self.frontmatter.sources(), &self.body)
    }

    /// The `# Computation` code block from the body, if there is one (§10.3).
    #[must_use]
    pub fn inline_computation(&self) -> Option<InlineComputation> {
        crate::computation::extract_inline_computation(&self.body)
    }

    /// The Attested Computation contract: the computation frontmatter (§10.2)
    /// resolved against the body's `# Computation` block (§10.3).
    ///
    /// Returns `None` unless `type` is `Attested Computation`; call
    /// [`AttestedComputation::from_parts`] directly to read the same keys off a
    /// concept of another type.
    #[must_use]
    pub fn attested_computation(&self) -> Option<AttestedComputation> {
        self.frontmatter
            .is_attested_computation()
            .then(|| AttestedComputation::from_parts(&self.frontmatter, &self.body))
    }

    /// Extracts numbered entries from a legacy v0.1 `# Citations` section.
    ///
    /// v0.2 supersedes this with `sources` and footnote attribution (§5.1);
    /// [`Document::attributions`] is the v0.2 equivalent. Consumers MAY keep
    /// reading `# Citations` for v0.1 documents (§13.1).
    #[must_use]
    pub fn citations(&self) -> Vec<Citation> {
        links::extract_citations(&self.body)
    }

    /// `true` when the body carries a legacy `# Citations` section, which a
    /// v0.2 producer should have migrated to `sources` (§13.1).
    #[must_use]
    pub fn has_legacy_citations(&self) -> bool {
        !self.citations().is_empty()
    }
}
