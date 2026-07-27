//! Provenance: the `sources` frontmatter family and per-claim attribution
//! (§5.1).
//!
//! `sources` records the materials a concept derives from, external or internal
//! to the bundle, together with the *credibility signals* (`author`,
//! `usage_count`, `last_modified`) a consumer needs to judge how far to trust
//! what was extracted from them.
//!
//! ```yaml
//! sources:
//!   - id: ga4-schema
//!     resource: https://developers.google.com/analytics/bigquery/export-schema
//!     title: GA4 BigQuery Export schema
//!     author: team:ga4-docs
//!     usage_count: 5000
//!     last_modified: 2026-05-30
//! usage_window: { from: 2026-06-01, to: 2026-06-30 }
//! ```
//!
//! Two design points from the spec show up directly in this module's API:
//!
//! - **No credibility score.** OKF stores objective signals, not a verdict: a
//!   score is subjective, unportable, and goes stale. So there is no
//!   `Source::score()`; credibility is inferred by the consumer from the
//!   signals, the way [`TrustTier`](crate::trust::TrustTier) is inferred from
//!   `verified`.
//! - **Keyed, not positional, attribution.** A claim cites a source by footnote
//!   label matching `sources[].id`, because agents constantly rewrite these
//!   documents and a positional index (`sources[0]`) misattributes silently the
//!   moment the list is reordered. [`attributions`] performs that join.

use crate::actor::Actor;
use crate::date::DateField;
use crate::footnotes;
use crate::yaml::Value;
use std::fmt;

/// The date range that frames `usage_count` (§5.1).
///
/// Written once as a sibling of `sources`; a single entry MAY carry its own to
/// override the shared one.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UsageWindow {
    /// Start of the window.
    pub from: Option<DateField>,
    /// End of the window.
    pub to: Option<DateField>,
}

impl UsageWindow {
    /// Reads a `{ from, to }` mapping. Returns `None` when the value is not a
    /// mapping.
    pub fn from_value(value: &Value) -> Option<Self> {
        let map = value.as_mapping()?;
        Some(Self {
            from: map
                .get("from")
                .and_then(Value::as_display_string)
                .map(DateField::new),
            to: map
                .get("to")
                .and_then(Value::as_display_string)
                .map(DateField::new),
        })
    }
}

impl fmt::Display for UsageWindow {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let dash = |d: &Option<DateField>| {
            d.as_ref()
                .map_or_else(|| "?".to_string(), |d| d.raw.clone())
        };
        write!(f, "{} to {}", dash(&self.from), dash(&self.to))
    }
}

/// What kind of thing a `sources[].resource` names (§5.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResourceKind {
    /// An absolute URL.
    Url,
    /// A path a consumer can follow into the bundle (or a `references/` file).
    Path,
    /// A population or scope descriptor a consumer cannot follow, such as
    /// `all queries in BigQuery project X`.
    Scope,
    /// No `resource` was given, so the entry is malformed, since `resource` is
    /// REQUIRED within an entry.
    Missing,
}

/// One entry in the `sources` list: a material the concept derives from.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Source {
    /// A stable key used to attribute individual claims. SHOULD be present when
    /// the body cites the source.
    pub id: Option<String>,
    /// REQUIRED within an entry: a concrete artifact or a scope descriptor.
    pub resource: Option<String>,
    /// Human-readable label for the source.
    pub title: Option<String>,
    /// Who or what produced the source, in the actor convention (§7). An
    /// authority signal.
    pub author: Option<Actor>,
    /// How often `resource` was exercised over the usage window. An adoption
    /// and liveness signal: coarse, and not a cross-kind ranking.
    pub usage_count: Option<i64>,
    /// When the source itself last changed. A recency signal, distinct from
    /// `generated.at` (which records when the *concept* was written).
    pub last_modified: Option<DateField>,
    /// An entry-level override of the shared `usage_window`.
    pub usage_window: Option<UsageWindow>,
}

impl Source {
    /// Reads one `sources` entry. Returns `None` when the value is not a
    /// mapping.
    pub fn from_value(value: &Value) -> Option<Self> {
        let map = value.as_mapping()?;
        let string = |k: &str| map.get(k).and_then(Value::as_display_string);
        Some(Self {
            id: string("id"),
            resource: string("resource"),
            title: string("title"),
            author: string("author").map(Actor::parse),
            usage_count: map.get("usage_count").and_then(Value::as_int),
            last_modified: string("last_modified").map(DateField::new),
            usage_window: map.get("usage_window").and_then(UsageWindow::from_value),
        })
    }

    /// Reads a whole `sources` value into a list of entries.
    ///
    /// A bare mapping is accepted as a one-element list, mirroring the rule
    /// §5.2 states for `verified`; any other shape yields an empty list.
    pub fn list_from_value(value: &Value) -> Vec<Self> {
        match value {
            Value::Sequence(items) => items.iter().filter_map(Self::from_value).collect(),
            Value::Mapping(_) => Self::from_value(value).into_iter().collect(),
            _ => Vec::new(),
        }
    }

    /// Classifies [`Source::resource`].
    ///
    /// Distinguishing a path from a scope descriptor is a heuristic (the spec
    /// gives no syntax for either), so a resource containing whitespace is read
    /// as a scope descriptor (`all queries in BigQuery project X`) and anything
    /// else as a path. Consumers that only follow [`ResourceKind::Path`]
    /// resources therefore never chase prose.
    pub fn resource_kind(&self) -> ResourceKind {
        match self.resource.as_deref().map(str::trim) {
            None | Some("") => ResourceKind::Missing,
            Some(r) if r.contains("://") || r.starts_with("mailto:") => ResourceKind::Url,
            Some(r) if r.chars().any(char::is_whitespace) => ResourceKind::Scope,
            Some(_) => ResourceKind::Path,
        }
    }

    /// The usage window that frames this entry's `usage_count`: its own if it
    /// has one, otherwise the shared sibling of `sources`.
    #[must_use]
    pub fn effective_usage_window<'a>(
        &'a self,
        shared: Option<&'a UsageWindow>,
    ) -> Option<&'a UsageWindow> {
        self.usage_window.as_ref().or(shared)
    }

    /// A short display label: the title, else the resource, else the id.
    #[must_use]
    pub fn label(&self) -> &str {
        self.title
            .as_deref()
            .or(self.resource.as_deref())
            .or(self.id.as_deref())
            .unwrap_or("(unnamed source)")
    }
}

impl fmt::Display for Source {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.id {
            Some(id) => write!(f, "[{id}] {}", self.label()),
            None => f.write_str(self.label()),
        }
    }
}

/// A body claim attributed to a source, produced by joining footnote labels to
/// `sources[].id` (§5.1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Attribution {
    /// The footnote label, which is the join key.
    pub label: String,
    /// The matching `sources` entry, or `None` when the label names no source.
    pub source: Option<Source>,
    /// How many times the body cites this label.
    pub references: usize,
    /// How many `[^label]: …` definition lines the body carries for it.
    pub definitions: usize,
}

impl Attribution {
    /// `true` when the label resolves to a `sources` entry.
    #[must_use]
    pub const fn is_resolved(&self) -> bool {
        self.source.is_some()
    }
}

/// Joins the body's footnotes to `sources` by label (§5.1).
///
/// Every label that appears as a reference or a definition gets one entry, in
/// order of first appearance. A label with no matching `sources[].id` still
/// appears, with [`Attribution::source`] set to `None`: an unresolvable
/// attribution is a producer mistake to report, not grounds for rejecting the
/// document (§11).
#[must_use]
pub fn attributions(sources: &[Source], body: &str) -> Vec<Attribution> {
    let refs = footnotes::extract_refs(body);
    let defs = footnotes::extract_definitions(body);

    let mut order: Vec<String> = Vec::new();
    let push = |label: &str, order: &mut Vec<String>| {
        if !order.iter().any(|l| l == label) {
            order.push(label.to_string());
        }
    };
    for r in &refs {
        push(&r.label, &mut order);
    }
    for d in &defs {
        push(&d.label, &mut order);
    }

    order
        .into_iter()
        .map(|label| Attribution {
            references: refs.iter().filter(|r| r.label == label).count(),
            definitions: defs.iter().filter(|d| d.label == label).count(),
            source: sources
                .iter()
                .find(|s| s.id.as_deref() == Some(label.as_str()))
                .cloned(),
            label,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::date::Date;

    const SOURCES: &str = "\
- id: rev-policy
  resource: https://wiki.acme/finance/revenue-recognition
  title: Revenue recognition policy
  author: team:finance-fpa
  last_modified: 2026-04-02
- id: exec-rev-dash
  resource: dashboards/exec-revenue
  title: Executive revenue dashboard
  author: team:finance-fpa
  usage_count: 5000
  last_modified: 2026-06-18
";

    fn sources() -> Vec<Source> {
        Source::list_from_value(&Value::parse(SOURCES).unwrap())
    }

    #[test]
    fn reads_entries_and_credibility_signals() {
        let s = sources();
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].id.as_deref(), Some("rev-policy"));
        assert_eq!(s[0].resource_kind(), ResourceKind::Url);
        assert_eq!(s[0].author.as_ref().unwrap().as_str(), "team:finance-fpa");
        assert_eq!(
            s[0].last_modified.as_ref().unwrap().date,
            Date::new(2026, 4, 2)
        );
        assert_eq!(s[0].usage_count, None);

        assert_eq!(s[1].usage_count, Some(5000));
        assert_eq!(s[1].resource_kind(), ResourceKind::Path);
        assert_eq!(s[1].label(), "Executive revenue dashboard");
    }

    #[test]
    fn scope_descriptors_are_not_paths() {
        let s = Source::from_value(
            &Value::parse("{ resource: all queries in BigQuery project X }").unwrap(),
        )
        .unwrap();
        assert_eq!(s.resource_kind(), ResourceKind::Scope);

        let missing = Source::from_value(&Value::parse("{ id: x }").unwrap()).unwrap();
        assert_eq!(missing.resource_kind(), ResourceKind::Missing);
    }

    #[test]
    fn usage_window_entry_overrides_shared() {
        let shared =
            UsageWindow::from_value(&Value::parse("{ from: 2026-06-01, to: 2026-06-30 }").unwrap())
                .unwrap();
        let plain = &sources()[1];
        assert_eq!(plain.effective_usage_window(Some(&shared)), Some(&shared));

        let overridden = Source::from_value(
            &Value::parse("{ resource: x, usage_window: { from: 2026-01-01, to: 2026-01-31 } }")
                .unwrap(),
        )
        .unwrap();
        let window = overridden.effective_usage_window(Some(&shared)).unwrap();
        assert_eq!(window.from.as_ref().unwrap().raw, "2026-01-01");
    }

    #[test]
    fn attribution_joins_footnote_labels_to_source_ids() {
        let body = "Per the recognition policy,[^rev-policy] corroborated by the \
                    dashboard.[^exec-rev-dash] And once more.[^rev-policy]\n\n\
                    [^rev-policy]: Revenue recognition policy\n\
                    [^exec-rev-dash]: Executive revenue dashboard\n\
                    [^ghost]: Not in sources\n";
        let attributions = attributions(&sources(), body);
        assert_eq!(attributions.len(), 3);

        assert_eq!(attributions[0].label, "rev-policy");
        assert_eq!(attributions[0].references, 2);
        assert_eq!(attributions[0].definitions, 1);
        assert!(attributions[0].is_resolved());
        assert_eq!(
            attributions[0].source.as_ref().unwrap().title.as_deref(),
            Some("Revenue recognition policy")
        );

        // A label with no matching source is reported, not dropped.
        assert_eq!(attributions[2].label, "ghost");
        assert_eq!(attributions[2].references, 0);
        assert!(!attributions[2].is_resolved());
    }

    #[test]
    fn reordering_sources_does_not_change_attribution() {
        let body = "Claim.[^exec-rev-dash]\n\n[^exec-rev-dash]: Executive revenue dashboard\n";
        let mut reversed = sources();
        reversed.reverse();
        let a = attributions(&sources(), body);
        let b = attributions(&reversed, body);
        assert_eq!(a, b);
        assert_eq!(
            a[0].source.as_ref().unwrap().id.as_deref(),
            Some("exec-rev-dash")
        );
    }
}
