//! The immutable world model: one internally consistent view of the bundle,
//! rebuilt in whole on the worker thread.
//!
//! Everything the UI shows comes from here — no widget ever calls
//! [`Bundle::load`] or touches the disk. Parse errors are not an error state:
//! broken files appear in the tree with a marker, matching okf-core's
//! permissive-loading design.

use crate::graph::GraphModel;
use crate::search::{SearchEntry, SearchIndex};
use okf_core::log::{Log, LogEntry};
use okf_core::{
    ActorKind, AttestedComputation, Bundle, BundleError, ComputationSource, ConceptId, Date,
    DateTimeField, Status, TrustTier,
};
use okf_validator::{Report, check_syntax, lint_bundle_at, validate_bundle_at};
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

/// Precomputed per-concept facts, so rendering is pure lookup.
#[derive(Clone, Debug)]
pub struct ConceptMeta {
    /// Trust tier.
    pub tier: TrustTier,
    /// Lifecycle status.
    pub status: Status,
    /// Whether the concept is stale on the snapshot's `today`.
    pub stale: bool,
    /// Days until `stale_after` when it lies within the next 30 days.
    pub stale_in_days: Option<i64>,
    /// Days since `stale_after` passed, when stale.
    pub overdue_days: Option<i64>,
    /// The raw `stale_after` field.
    pub stale_after: Option<DateTimeField>,
    /// Validation errors attributed to this concept.
    pub diag_errors: usize,
    /// Validation warnings attributed to this concept.
    pub diag_warnings: usize,
    /// Lint findings attributed to this concept.
    pub lint_findings: usize,
    /// Outgoing link count.
    pub out_degree: usize,
    /// Incoming link count.
    pub in_degree: usize,
    /// Outgoing links whose target does not exist.
    pub broken_out: usize,
    /// Number of `sources` entries.
    pub source_count: usize,
    /// Whether the concept is an Attested Computation.
    pub is_computation: bool,
    /// Body headings, as `(level, text)`.
    pub headings: Vec<(usize, String)>,
}

/// Aggregated activity of one actor across the bundle.
#[derive(Clone, Debug, Default)]
pub struct ActorStats {
    /// The actor's kind.
    pub kind: Option<ActorKind>,
    /// How many concepts this actor `generated`.
    pub generated: usize,
    /// How many verification events this actor signed.
    pub verified: usize,
}

/// The `okf info` numbers, precomputed for the status bar and dashboards.
#[derive(Clone, Debug, Default)]
pub struct BundleStats {
    /// Total concepts.
    pub concepts: usize,
    /// Files that failed to parse.
    pub parse_errors: usize,
    /// Concept counts by trust tier: `[unverified, machine, human]`.
    pub tier_counts: [usize; 3],
    /// Concept counts by status: `[draft, stable, deprecated, other]`.
    pub status_counts: [usize; 4],
    /// Concepts stale today.
    pub stale: usize,
    /// Concepts going stale within 30 days.
    pub stale_soon: usize,
    /// Broken outgoing links across the bundle.
    pub broken_links: usize,
    /// Concept counts by `type`.
    pub types: BTreeMap<String, usize>,
    /// Actor activity, aggregated from `generated.by` and `verified[].by`.
    pub actors: BTreeMap<String, ActorStats>,
}

/// One row of the mission-control attention queue.
#[derive(Clone, Debug)]
pub struct AttentionItem {
    /// The concept needing a decision.
    pub id: ConceptId,
    /// The transparent risk score the queue ranks by.
    pub risk: i64,
    /// Why the row ranks where it does.
    pub reasons: Vec<String>,
}

/// A precomputed Attested Computation contract with its health checks.
#[derive(Clone, Debug)]
pub struct ContractInfo {
    /// The computation concept's id.
    pub id: ConceptId,
    /// Display title.
    pub title: String,
    /// The typed contract.
    pub contract: AttestedComputation,
    /// Path-valued fields with their resolution result.
    pub path_checks: Vec<(String, String, Option<PathBuf>)>,
    /// Inline-code syntax verdict, when the computation is inline and the
    /// runtime names a checkable language.
    pub syntax: Option<Result<(), String>>,
    /// Itemized failures; empty means the ✓ aggregate.
    pub issues: Vec<String>,
}

impl ContractInfo {
    /// `true` when every health check passes.
    #[must_use]
    pub const fn healthy(&self) -> bool {
        self.issues.is_empty()
    }
}

/// A leaf in the file tree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LeafKind {
    /// A parsed concept.
    Concept(ConceptId),
    /// A reserved `index.md`.
    Index(PathBuf),
    /// A reserved `log.md`.
    Log(PathBuf),
    /// A file whose frontmatter failed to parse.
    Broken(PathBuf),
}

/// One node of the file tree.
#[derive(Clone, Debug)]
pub enum TreeNode {
    /// A directory with children.
    Dir {
        /// The directory's own name.
        name: String,
        /// The `/`-joined path from the bundle root, the collapse key.
        path: String,
        /// Recursive concept count.
        concept_count: usize,
        /// Child nodes, reserved files first, then dirs, then leaves.
        children: Vec<Self>,
    },
    /// A file.
    Leaf {
        /// Display name (filename).
        name: String,
        /// What the file is.
        kind: LeafKind,
    },
}

/// The bundle's directory tree.
#[derive(Clone, Debug, Default)]
pub struct FileTree {
    /// Top-level nodes.
    pub roots: Vec<TreeNode>,
}

/// One immutable, internally consistent view of the world.
#[derive(Debug)]
pub struct Snapshot {
    /// Monotonically increasing rebuild counter.
    pub generation: u64,
    /// The pinned or wall-clock date staleness is evaluated against.
    pub today: Date,
    /// The loaded bundle, as-is.
    pub bundle: Bundle,
    /// Conformance findings.
    pub validation: Report,
    /// Lint findings.
    pub lint: Report,
    /// The directory tree.
    pub tree: FileTree,
    /// Per-concept precomputed facts.
    pub concept_meta: HashMap<ConceptId, ConceptMeta>,
    /// The link graph model.
    pub graph: GraphModel,
    /// The omnisearch index.
    pub search: SearchIndex,
    /// Merged log activity: entry count per parseable date, ascending.
    pub log_timeline: Vec<(Date, usize)>,
    /// Merged log entries, newest date first, for the full log view.
    pub log_days: Vec<(String, Vec<LogEntry>)>,
    /// The `okf info` numbers.
    pub stats: BundleStats,
    /// The mission-control attention queue, highest risk first.
    pub attention: Vec<AttentionItem>,
    /// Attested Computation contracts with health checks.
    pub contracts: Vec<ContractInfo>,
}

impl Snapshot {
    /// Loads and derives a full snapshot.
    ///
    /// # Errors
    ///
    /// Returns [`BundleError`] only for I/O failures or a non-directory root;
    /// per-file parse failures land in the tree and diagnostics instead.
    pub fn build(
        root: &std::path::Path,
        today: Option<Date>,
        generation: u64,
    ) -> Result<Self, BundleError> {
        let bundle = Bundle::load(root)?;
        let today = today.or_else(Date::today_utc).unwrap_or(Date {
            year: 2026,
            month: 1,
            day: 1,
        });
        let validation = validate_bundle_at(&bundle, Some(today));
        let lint = lint_bundle_at(&bundle, Some(today));

        let concept_meta = build_meta(&bundle, &validation, &lint, today);
        let tree = build_tree(&bundle);
        let graph = GraphModel::build(&bundle);
        let search = build_search(&bundle, &concept_meta);
        let (log_timeline, log_days) = build_log_views(&bundle);
        let stats = build_stats(&bundle, &concept_meta);
        let attention = build_attention(&bundle, &concept_meta);
        let contracts = build_contracts(&bundle);

        Ok(Self {
            generation,
            today,
            bundle,
            validation,
            lint,
            tree,
            concept_meta,
            graph,
            search,
            log_timeline,
            log_days,
            stats,
            attention,
            contracts,
        })
    }

    /// The meta record for a concept.
    #[must_use]
    pub fn meta(&self, id: &ConceptId) -> Option<&ConceptMeta> {
        self.concept_meta.get(id)
    }
}

fn days_between(from: Date, to: Date) -> i64 {
    to.days_since_epoch() - from.days_since_epoch()
}

fn build_meta(
    bundle: &Bundle,
    validation: &Report,
    lint: &Report,
    today: Date,
) -> HashMap<ConceptId, ConceptMeta> {
    let mut meta = HashMap::new();
    for concept in bundle.concepts() {
        let fm = &concept.document.frontmatter;
        let stale_after = fm.stale_after();
        let stale = concept.is_stale_on(today);
        let effective = stale_after.as_ref().and_then(|f| {
            f.datetime
                .map(|dt| dt.utc_date())
                .or_else(|| Date::parse(f.raw.trim().get(..10).unwrap_or("")))
        });
        let (stale_in_days, overdue_days) = effective.map_or((None, None), |date| {
            let delta = days_between(today, date);
            if stale {
                (None, Some(-delta))
            } else if (0..=30).contains(&delta) {
                (Some(delta), None)
            } else {
                (None, None)
            }
        });
        let links = bundle.links_from(&concept.id);
        let count_for = |report: &Report, severity: Option<okf_validator::Severity>| {
            report
                .diagnostics
                .iter()
                .filter(|d| {
                    d.concept.as_ref() == Some(&concept.id)
                        || d.path.as_ref() == Some(&concept.path)
                })
                .filter(|d| severity.is_none_or(|s| d.severity == s))
                .count()
        };
        meta.insert(
            concept.id.clone(),
            ConceptMeta {
                tier: concept.trust_tier(),
                status: concept.status(),
                stale,
                stale_in_days,
                overdue_days,
                stale_after,
                diag_errors: count_for(validation, Some(okf_validator::Severity::Error)),
                diag_warnings: count_for(validation, Some(okf_validator::Severity::Warning)),
                lint_findings: count_for(lint, None),
                out_degree: links.len(),
                in_degree: bundle.backlinks(&concept.id).len(),
                broken_out: links.iter().filter(|l| !l.exists).count(),
                source_count: concept.sources().len(),
                is_computation: concept.document.frontmatter.is_attested_computation(),
                headings: okf_core::extract_headings(&concept.document.body)
                    .iter()
                    .map(|h| (h.level, h.text.to_string()))
                    .collect(),
            },
        );
    }
    meta
}

#[allow(clippy::too_many_lines)]
fn build_tree(bundle: &Bundle) -> FileTree {
    /// Intermediate mutable directory.
    #[derive(Default)]
    struct DirBuilder {
        dirs: BTreeMap<String, Self>,
        leaves: Vec<(String, LeafKind)>,
        concepts: usize,
    }
    fn insert(root: &mut DirBuilder, segments: &[String], leaf: (String, LeafKind)) {
        let is_concept = matches!(leaf.1, LeafKind::Concept(_));
        let mut cur = root;
        if is_concept {
            cur.concepts += 1;
        }
        for seg in segments {
            cur = cur.dirs.entry(seg.clone()).or_default();
            if is_concept {
                cur.concepts += 1;
            }
        }
        cur.leaves.push(leaf);
    }
    fn finish(builder: DirBuilder, prefix: &str) -> Vec<TreeNode> {
        let mut reserved: Vec<TreeNode> = Vec::new();
        let mut leaves: Vec<TreeNode> = Vec::new();
        let mut sorted = builder.leaves;
        sorted.sort_by(|a, b| a.0.cmp(&b.0));
        for (name, kind) in sorted {
            let node = TreeNode::Leaf { name, kind };
            match &node {
                TreeNode::Leaf {
                    kind: LeafKind::Index(_) | LeafKind::Log(_),
                    ..
                } => reserved.push(node),
                _ => leaves.push(node),
            }
        }
        reserved.sort_by_key(|n| match n {
            TreeNode::Leaf {
                kind: LeafKind::Index(_),
                ..
            } => 0,
            _ => 1,
        });
        let mut dirs: Vec<TreeNode> = Vec::new();
        for (name, child) in builder.dirs {
            let path = if prefix.is_empty() {
                name.clone()
            } else {
                format!("{prefix}/{name}")
            };
            let concept_count = child.concepts;
            let children = finish(child, &path);
            dirs.push(TreeNode::Dir {
                name,
                path,
                concept_count,
                children,
            });
        }
        let mut out = reserved;
        out.extend(dirs);
        out.extend(leaves);
        out
    }

    let mut root = DirBuilder::default();
    let rel_segments = |path: &std::path::Path| -> Option<(Vec<String>, String)> {
        let rel = path.strip_prefix(bundle.root()).ok()?;
        let mut segs: Vec<String> = rel
            .components()
            .filter_map(|c| match c {
                std::path::Component::Normal(s) => Some(s.to_string_lossy().into_owned()),
                _ => None,
            })
            .collect();
        let name = segs.pop()?;
        Some((segs, name))
    };
    for concept in bundle.concepts() {
        let segments = concept.id.segments();
        let dirs = segments[..segments.len() - 1].to_vec();
        insert(
            &mut root,
            &dirs,
            (
                format!("{}.md", concept.id.name()),
                LeafKind::Concept(concept.id.clone()),
            ),
        );
    }
    for path in bundle.index_files() {
        if let Some((dirs, name)) = rel_segments(path) {
            insert(&mut root, &dirs, (name, LeafKind::Index(path.clone())));
        }
    }
    for path in bundle.log_files() {
        if let Some((dirs, name)) = rel_segments(path) {
            insert(&mut root, &dirs, (name, LeafKind::Log(path.clone())));
        }
    }
    for (path, _) in bundle.parse_errors() {
        if let Some((dirs, name)) = rel_segments(path) {
            insert(&mut root, &dirs, (name, LeafKind::Broken(path.clone())));
        }
    }
    FileTree {
        roots: finish(root, ""),
    }
}

fn build_search(bundle: &Bundle, meta: &HashMap<ConceptId, ConceptMeta>) -> SearchIndex {
    let entries = bundle
        .concepts()
        .iter()
        .map(|concept| {
            let m = meta.get(&concept.id);
            SearchEntry {
                id: concept.id.clone(),
                title: concept.display_title(),
                description: concept
                    .document
                    .frontmatter
                    .description()
                    .map(std::borrow::Cow::into_owned)
                    .unwrap_or_default(),
                tags: concept.document.frontmatter.tags(),
                headings: m
                    .map(|m| m.headings.iter().map(|(_, t)| t.clone()).collect())
                    .unwrap_or_default(),
                type_: concept
                    .type_()
                    .map(std::borrow::Cow::into_owned)
                    .unwrap_or_default(),
                tier: concept.trust_tier(),
                status: concept.status(),
                stale: m.is_some_and(|m| m.stale),
                broken: m.is_some_and(|m| m.broken_out > 0),
            }
        })
        .collect();
    SearchIndex { entries }
}

/// The two log-derived views: the per-date activity counts and the merged
/// newest-first day groups.
type LogViews = (Vec<(Date, usize)>, Vec<(String, Vec<LogEntry>)>);

fn build_log_views(bundle: &Bundle) -> LogViews {
    let mut counts: BTreeMap<Date, usize> = BTreeMap::new();
    let mut days: BTreeMap<String, Vec<LogEntry>> = BTreeMap::new();
    for path in bundle.log_files() {
        let Ok(text) = std::fs::read_to_string(path) else {
            continue;
        };
        let log = Log::parse(&text);
        for day in log.days {
            if let Some(date) = Date::parse(day.date.trim()) {
                *counts.entry(date).or_default() += day.entries.len();
            }
            days.entry(day.date.clone())
                .or_default()
                .extend(day.entries);
        }
    }
    let timeline = counts.into_iter().collect();
    // Newest first; non-date headings sort after real dates.
    let mut merged: Vec<(String, Vec<LogEntry>)> = days.into_iter().collect();
    merged.sort_by(|a, b| b.0.cmp(&a.0));
    (timeline, merged)
}

fn build_stats(bundle: &Bundle, meta: &HashMap<ConceptId, ConceptMeta>) -> BundleStats {
    let mut stats = BundleStats {
        concepts: bundle.len(),
        parse_errors: bundle.parse_errors().len(),
        broken_links: bundle.broken_links().len(),
        ..BundleStats::default()
    };
    for concept in bundle.concepts() {
        let m = &meta[&concept.id];
        stats.tier_counts[match m.tier {
            TrustTier::Unverified => 0,
            TrustTier::MachineConfirmed => 1,
            TrustTier::HumanReviewed => 2,
        }] += 1;
        stats.status_counts[match m.status {
            Status::Draft => 0,
            Status::Stable => 1,
            Status::Deprecated => 2,
            Status::Other(_) => 3,
        }] += 1;
        if m.stale {
            stats.stale += 1;
        } else if m.stale_in_days.is_some() {
            stats.stale_soon += 1;
        }
        let type_ = concept
            .type_()
            .map_or_else(|| "(untyped)".to_string(), std::borrow::Cow::into_owned);
        *stats.types.entry(type_).or_default() += 1;

        let fm = &concept.document.frontmatter;
        if let Some(by) = fm.generated().and_then(|g| g.by) {
            let entry = stats.actors.entry(by.as_str().to_string()).or_default();
            entry.kind = Some(by.kind());
            entry.generated += 1;
        }
        for verification in fm.verified() {
            if let Some(by) = verification.by {
                let entry = stats.actors.entry(by.as_str().to_string()).or_default();
                entry.kind = Some(by.kind());
                entry.verified += 1;
            }
        }
    }
    stats
}

fn build_attention(bundle: &Bundle, meta: &HashMap<ConceptId, ConceptMeta>) -> Vec<AttentionItem> {
    let mut items = Vec::new();
    for concept in bundle.concepts() {
        let m = &meta[&concept.id];
        let mut risk: i64 = 0;
        let mut reasons: Vec<String> = Vec::new();
        if let Some(overdue) = m.overdue_days {
            risk += 40 + overdue.clamp(0, 60);
            reasons.push(format!("stale {overdue}d"));
        } else if let Some(days) = m.stale_in_days {
            risk += 10 + (30 - days).max(0);
            reasons.push(format!("stale in {days}d"));
        } else if m.stale {
            risk += 40;
            reasons.push("stale".to_string());
        }
        match m.tier {
            TrustTier::Unverified => {
                risk += 25;
                reasons.push("unverified".to_string());
            }
            TrustTier::MachineConfirmed => {
                risk += 10;
                reasons.push("machine-confirmed".to_string());
            }
            TrustTier::HumanReviewed => reasons.push("human-reviewed".to_string()),
        }
        if m.status.is_deprecated() && m.in_degree > 0 {
            risk += 20;
            reasons.push(format!(
                "deprecated · {} incoming links remain",
                m.in_degree
            ));
        }
        if m.in_degree > 0 {
            risk += i64::try_from(m.in_degree).unwrap_or(0).min(10) * 3;
            reasons.push(format!("{} backlinks", m.in_degree));
        }
        let diags = m.diag_errors * 15 + m.diag_warnings * 5 + m.lint_findings * 2;
        if diags > 0 {
            risk += i64::try_from(diags).unwrap_or(0);
            reasons.push(format!(
                "{} diagnostic(s)",
                m.diag_errors + m.diag_warnings + m.lint_findings
            ));
        }
        let needs_attention = m.stale
            || m.stale_in_days.is_some()
            || m.tier == TrustTier::Unverified
            || (m.status.is_deprecated() && m.in_degree > 0)
            || m.diag_errors + m.diag_warnings > 0;
        if needs_attention {
            items.push(AttentionItem {
                id: concept.id.clone(),
                risk,
                reasons,
            });
        }
    }
    items.sort_by(|a, b| b.risk.cmp(&a.risk).then_with(|| a.id.cmp(&b.id)));
    items
}

fn build_contracts(bundle: &Bundle) -> Vec<ContractInfo> {
    bundle
        .attested_computations()
        .filter_map(|concept| {
            let contract = concept.attested_computation()?;
            let mut issues = Vec::new();
            if contract.runtime.is_none() {
                issues.push("missing required `runtime`".to_string());
            }
            if contract.computation.is_missing() {
                issues.push(
                    "no computation: neither inline block nor `computation` path".to_string(),
                );
            }
            if contract.has_redundant_inline {
                issues.push("both `computation` path and inline block present".to_string());
            }
            for parameter in &contract.parameters {
                if parameter.name.is_none() {
                    issues.push("a parameter entry lacks `name`".to_string());
                }
            }
            let mut path_checks = Vec::new();
            for (field, raw) in contract.path_fields() {
                let resolved = bundle.resolve_path_field(&concept.id, raw);
                if resolved.is_none() {
                    issues.push(format!("{field} does not resolve: {raw}"));
                }
                path_checks.push((field.to_string(), raw.to_string(), resolved));
            }
            let syntax = match (&contract.computation, contract.runtime.as_deref()) {
                (ComputationSource::Inline(inline), runtime) => {
                    let tag = inline
                        .language
                        .clone()
                        .or_else(|| runtime.map(String::from));
                    tag.map(|tag| check_syntax(&tag, &inline.code).map_err(|e| e.to_string()))
                }
                _ => None,
            };
            if let Some(Err(e)) = &syntax {
                issues.push(format!("computation syntax: {e}"));
            }
            Some(ContractInfo {
                id: concept.id.clone(),
                title: concept.display_title(),
                contract,
                path_checks,
                syntax,
                issues,
            })
        })
        .collect()
}
