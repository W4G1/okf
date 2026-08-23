//! Integration tests for `okf lint`.
//!
//! Each test pins one rule against a minimal fixture, so a regression points
//! at the rule that broke. The fixtures are deliberately tiny and independent
//! of the spec's Appendix A bundle, which `conformance.rs` already covers.

mod common;

use common::TempDir;
use okf_core::{Bundle, Date};
use okf_validator::{Severity, lint_bundle, lint_bundle_at};

/// Returns the messages of every finding with the given rule code (without
/// the `[code] ` prefix).
fn messages_for<'a>(report: &'a okf_validator::validate::Report, code: &'a str) -> Vec<&'a str> {
    let prefix = format!("[{code}] ");
    report
        .diagnostics
        .iter()
        .filter_map(move |d| d.message.strip_prefix(&prefix))
        .collect()
}

fn has(report: &okf_validator::validate::Report, code: &str) -> bool {
    report
        .diagnostics
        .iter()
        .any(|d| d.message.starts_with(&format!("[{code}] ")))
}

/// A minimal conformant concept. Tests add or remove fields to trigger the
/// rule under test.
const MIN_CONCEPT: &str = "---\n\
type: Metric\n\
title: Revenue\n\
description: Recognized revenue.\n\
generated: { by: reference_agent/gemini-2.5-pro, at: 2026-06-20T22:53:05Z }\n\
verified: { by: human:ahormati, at: 2026-06-25T09:00:00Z }\n\
---\n\n\
# Definition\n\n\
Recognized revenue.\n";

/// Writes `MIN_CONCEPT` to `metric.md` and loads the bundle.
fn minimal_bundle() -> (TempDir, Bundle) {
    let tmp = TempDir::new();
    tmp.write("metric.md", MIN_CONCEPT);
    // A root index.md keeps the concept from being flagged as an orphan
    // (L15), so the rule under test is the only finding.
    tmp.write("index.md", "# Metric\n\n* [Revenue](metric.md)\n");
    let bundle = Bundle::load(tmp.path()).unwrap();
    (tmp, bundle)
}

#[test]
fn a_filled_in_concept_lints_clean() {
    let (_tmp, bundle) = minimal_bundle();
    let report = lint_bundle(&bundle);
    let warnings: Vec<&str> = report
        .of(Severity::Warning)
        .map(|d| d.message.as_str())
        .collect();
    assert!(warnings.is_empty(), "unexpected warnings: {warnings:#?}");
}

#[test]
fn l1_missing_title() {
    let tmp = TempDir::new();
    tmp.write(
        "metric.md",
        "---\ntype: Metric\ndescription: Has no title.\n\
         generated: { by: ref/x, at: 2026-06-20T22:53:05Z }\n\
         verified: { by: human:a, at: 2026-06-25T09:00:00Z }\n\
         ---\n\n# Definition\n\nProse.\n",
    );
    tmp.write("index.md", "# Metric\n\n* [metric](metric.md)\n");
    let bundle = Bundle::load(tmp.path()).unwrap();
    assert!(has(&lint_bundle(&bundle), "L1"));
}

#[test]
fn l2_missing_description() {
    let tmp = TempDir::new();
    tmp.write(
        "metric.md",
        "---\ntype: Metric\ntitle: Revenue\n\
         generated: { by: ref/x, at: 2026-06-20T22:53:05Z }\n\
         verified: { by: human:a, at: 2026-06-25T09:00:00Z }\n\
         ---\n\n# Definition\n\nProse.\n",
    );
    tmp.write("index.md", "# Metric\n\n* [Revenue](metric.md)\n");
    let bundle = Bundle::load(tmp.path()).unwrap();
    assert!(has(&lint_bundle(&bundle), "L2"));
}

#[test]
fn l3_missing_generated() {
    let tmp = TempDir::new();
    tmp.write(
        "metric.md",
        "---\ntype: Metric\ntitle: Revenue\ndescription: d\n\
         verified: { by: human:a, at: 2026-06-25T09:00:00Z }\n\
         ---\n\n# Definition\n\nProse.\n",
    );
    tmp.write("index.md", "# Metric\n\n* [Revenue](metric.md)\n");
    let bundle = Bundle::load(tmp.path()).unwrap();
    assert!(has(&lint_bundle(&bundle), "L3"));
}

#[test]
fn l3_satisfied_by_legacy_timestamp() {
    let tmp = TempDir::new();
    tmp.write(
        "metric.md",
        "---\ntype: Metric\ntitle: Revenue\ndescription: d\n\
         timestamp: 2026-05-28T22:53:05+00:00\n\
         verified: { by: human:a, at: 2026-06-25T09:00:00Z }\n\
         ---\n\n# Definition\n\nProse.\n",
    );
    tmp.write("index.md", "# Metric\n\n* [Revenue](metric.md)\n");
    let bundle = Bundle::load(tmp.path()).unwrap();
    let report = lint_bundle(&bundle);
    assert!(
        !has(&report, "L3"),
        "timestamp should stand in for generated; L3 must not fire"
    );
    // L5 still fires, since the legacy key is itself worth migrating away from.
    assert!(has(&report, "L5"));
}

#[test]
fn l4_unverified_is_info() {
    let tmp = TempDir::new();
    tmp.write(
        "metric.md",
        "---\ntype: Metric\ntitle: Revenue\ndescription: d\n\
         generated: { by: ref/x, at: 2026-06-20T22:53:05Z }\n\
         ---\n\n# Definition\n\nProse.\n",
    );
    tmp.write("index.md", "# Metric\n\n* [Revenue](metric.md)\n");
    let bundle = Bundle::load(tmp.path()).unwrap();
    let report = lint_bundle(&bundle);
    assert!(has(&report, "L4"));
    let is_info = report
        .diagnostics
        .iter()
        .any(|d| d.severity == Severity::Info && d.message.starts_with("[L4] "));
    assert!(is_info, "L4 must be info, not warning");
}

#[test]
fn l5_legacy_timestamp() {
    let tmp = TempDir::new();
    tmp.write(
        "metric.md",
        "---\ntype: Metric\ntitle: Revenue\ndescription: d\n\
         timestamp: 2026-05-28T22:53:05+00:00\n\
         generated: { by: ref/x, at: 2026-06-20T22:53:05Z }\n\
         verified: { by: human:a, at: 2026-06-25T09:00:00Z }\n\
         ---\n\n# Definition\n\nProse.\n",
    );
    tmp.write("index.md", "# Metric\n\n* [Revenue](metric.md)\n");
    let bundle = Bundle::load(tmp.path()).unwrap();
    assert!(has(&lint_bundle(&bundle), "L5"));
}

#[test]
fn l6_legacy_citations() {
    let tmp = TempDir::new();
    tmp.write(
        "metric.md",
        "---\ntype: Metric\ntitle: Revenue\ndescription: d\n\
         generated: { by: ref/x, at: 2026-06-20T22:53:05Z }\n\
         verified: { by: human:a, at: 2026-06-25T09:00:00Z }\n\
         ---\n\n# Definition\n\nProse.\n\n\
         # Citations\n[1] [Policy](https://wiki.acme/policy)\n",
    );
    tmp.write("index.md", "# Metric\n\n* [Revenue](metric.md)\n");
    let bundle = Bundle::load(tmp.path()).unwrap();
    assert!(has(&lint_bundle(&bundle), "L6"));
}

#[test]
fn l7_empty_body() {
    let tmp = TempDir::new();
    tmp.write(
        "metric.md",
        "---\ntype: Metric\ntitle: Revenue\ndescription: d\n\
         generated: { by: ref/x, at: 2026-06-20T22:53:05Z }\n\
         verified: { by: human:a, at: 2026-06-25T09:00:00Z }\n\
         ---\n\n   \n",
    );
    tmp.write("index.md", "# Metric\n\n* [Revenue](metric.md)\n");
    let bundle = Bundle::load(tmp.path()).unwrap();
    assert!(has(&lint_bundle(&bundle), "L7"));
    // L8 must not also fire on an empty body, since L7 already covers it.
    let report = lint_bundle(&bundle);
    assert!(
        !has(&report, "L8"),
        "L8 should not fire when the body is empty (L7 already did)"
    );
}

#[test]
fn l8_no_top_heading() {
    let tmp = TempDir::new();
    tmp.write(
        "metric.md",
        "---\ntype: Metric\ntitle: Revenue\ndescription: d\n\
         generated: { by: ref/x, at: 2026-06-20T22:53:05Z }\n\
         verified: { by: human:a, at: 2026-06-25T09:00:00Z }\n\
         ---\n\nSome prose without a heading.\n",
    );
    tmp.write("index.md", "# Metric\n\n* [Revenue](metric.md)\n");
    let bundle = Bundle::load(tmp.path()).unwrap();
    assert!(has(&lint_bundle(&bundle), "L8"));
}

#[test]
fn l9_verified_before_generated() {
    let tmp = TempDir::new();
    tmp.write(
        "metric.md",
        "---\ntype: Metric\ntitle: Revenue\ndescription: d\n\
         generated: { by: ref/x, at: 2026-06-28T14:00:00Z }\n\
         verified: { by: human:a, at: 2026-06-25T09:00:00Z }\n\
         ---\n\n# Definition\n\nProse.\n",
    );
    tmp.write("index.md", "# Metric\n\n* [Revenue](metric.md)\n");
    let bundle = Bundle::load(tmp.path()).unwrap();
    let report = lint_bundle(&bundle);
    let msgs = messages_for(&report, "L9");
    assert!(
        msgs.iter().any(|m| m.contains("predates `generated.at`")),
        "{msgs:?}"
    );
}

#[test]
fn l9_does_not_fire_when_verification_is_current() {
    let (_tmp, bundle) = minimal_bundle();
    let report = lint_bundle(&bundle);
    assert!(
        !has(&report, "L9"),
        "verified (2026-06-25) is the same day as generated (2026-06-20): no L9"
    );
}

#[test]
fn l10_links_to_deprecated() {
    let tmp = TempDir::new();
    tmp.write(
        "old.md",
        "---\ntype: Metric\ntitle: Old\ndescription: d\nstatus: deprecated\n\
         generated: { by: ref/x, at: 2026-06-20T22:53:05Z }\n\
         verified: { by: human:a, at: 2026-06-25T09:00:00Z }\n\
         ---\n\n# Definition\n\nProse.\n",
    );
    tmp.write(
        "new.md",
        "---\ntype: Metric\ntitle: New\ndescription: d\n\
         generated: { by: ref/x, at: 2026-06-20T22:53:05Z }\n\
         verified: { by: human:a, at: 2026-06-25T09:00:00Z }\n\
         ---\n\n# Definition\n\nSee [the old one](old.md).\n",
    );
    tmp.write("index.md", "# Metric\n\n* [New](new.md)\n* [Old](old.md)\n");
    let bundle = Bundle::load(tmp.path()).unwrap();
    let report = lint_bundle(&bundle);
    let l10 = messages_for(&report, "L10");
    assert!(
        l10.iter().any(|m| m.contains("deprecated concept `old`")),
        "{l10:?}"
    );
}

#[test]
fn l11_stale_with_today() {
    let tmp = TempDir::new();
    tmp.write(
        "metric.md",
        "---\ntype: Metric\ntitle: Revenue\ndescription: d\n\
         stale_after: 2026-06-15T00:00:00Z\n\
         generated: { by: ref/x, at: 2026-06-01T00:00:00Z }\n\
         verified: { by: human:a, at: 2026-06-02T00:00:00Z }\n\
         ---\n\n# Definition\n\nProse.\n",
    );
    tmp.write("index.md", "# Metric\n\n* [Revenue](metric.md)\n");
    let bundle = Bundle::load(tmp.path()).unwrap();

    // Without --today, staleness is not evaluated, so L11 must not fire.
    let without_today = lint_bundle(&bundle);
    assert!(!has(&without_today, "L11"));

    // With --today past stale_after, L11 fires.
    let with_today = lint_bundle_at(&bundle, Date::new(2026, 7, 1));
    let l11 = messages_for(&with_today, "L11");
    assert!(
        l11.iter()
            .any(|m| m.contains("stale since 2026-06-15T00:00:00Z")),
        "{l11:?}"
    );
}

#[test]
fn l12_draft_status_is_info() {
    let tmp = TempDir::new();
    tmp.write(
        "metric.md",
        "---\ntype: Metric\ntitle: Revenue\ndescription: d\nstatus: draft\n\
         generated: { by: ref/x, at: 2026-06-20T22:53:05Z }\n\
         verified: { by: human:a, at: 2026-06-25T09:00:00Z }\n\
         ---\n\n# Definition\n\nProse.\n",
    );
    tmp.write("index.md", "# Metric\n\n* [Revenue](metric.md)\n");
    let bundle = Bundle::load(tmp.path()).unwrap();
    let report = lint_bundle(&bundle);
    assert!(has(&report, "L12"));
    let is_info = report
        .diagnostics
        .iter()
        .any(|d| d.severity == Severity::Info && d.message.starts_with("[L12] "));
    assert!(is_info, "L12 must be info, not warning");
}

#[test]
fn l13_self_link() {
    let tmp = TempDir::new();
    tmp.write(
        "metric.md",
        "---\ntype: Metric\ntitle: Revenue\ndescription: d\n\
         generated: { by: ref/x, at: 2026-06-20T22:53:05Z }\n\
         verified: { by: human:a, at: 2026-06-25T09:00:00Z }\n\
         ---\n\n# Definition\n\nSee [myself](metric.md).\n",
    );
    tmp.write("index.md", "# Metric\n\n* [Revenue](metric.md)\n");
    let bundle = Bundle::load(tmp.path()).unwrap();
    let report = lint_bundle(&bundle);
    assert!(has(&report, "L13"));
    let is_info = report
        .diagnostics
        .iter()
        .any(|d| d.severity == Severity::Info && d.message.starts_with("[L13] "));
    assert!(is_info, "L13 must be info");
}

#[test]
fn l14_duplicate_title() {
    let tmp = TempDir::new();
    for name in ["a.md", "b.md"] {
        tmp.write(
            name,
            "---\ntype: Metric\ntitle: Same title\ndescription: d\n\
             generated: { by: ref/x, at: 2026-06-20T22:53:05Z }\n\
             verified: { by: human:a, at: 2026-06-25T09:00:00Z }\n\
             ---\n\n# Definition\n\nProse.\n",
        );
    }
    tmp.write("index.md", "# Metric\n\n* [a](a.md)\n* [b](b.md)\n");
    let bundle = Bundle::load(tmp.path()).unwrap();
    let report = lint_bundle(&bundle);
    let l14 = messages_for(&report, "L14");
    assert_eq!(l14.len(), 2, "both concepts should be flagged: {l14:?}");
    assert!(l14.iter().all(|m| m.contains("`title` \"Same title\"")));
}

#[test]
fn l15_orphan_when_not_linked_or_indexed() {
    let tmp = TempDir::new();
    tmp.write(
        "metric.md",
        "---\ntype: Metric\ntitle: Revenue\ndescription: d\n\
         generated: { by: ref/x, at: 2026-06-20T22:53:05Z }\n\
         verified: { by: human:a, at: 2026-06-25T09:00:00Z }\n\
         ---\n\n# Definition\n\nProse.\n",
    );
    // No index.md, no inbound links: the concept is stranded.
    let bundle = Bundle::load(tmp.path()).unwrap();
    assert!(has(&lint_bundle(&bundle), "L15"));
}

#[test]
fn l15_rescinded_by_being_listed_in_an_index() {
    let tmp = TempDir::new();
    tmp.write(
        "metric.md",
        "---\ntype: Metric\ntitle: Revenue\ndescription: d\n\
         generated: { by: ref/x, at: 2026-06-20T22:53:05Z }\n\
         verified: { by: human:a, at: 2026-06-25T09:00:00Z }\n\
         ---\n\n# Definition\n\nProse.\n",
    );
    tmp.write("index.md", "# Metric\n\n* [Revenue](metric.md)\n");
    let bundle = Bundle::load(tmp.path()).unwrap();
    assert!(!has(&lint_bundle(&bundle), "L15"));
}

#[test]
fn l15_rescinded_by_an_inbound_link() {
    let tmp = TempDir::new();
    tmp.write(
        "cited.md",
        "---\ntype: Metric\ntitle: Cited\ndescription: d\n\
         generated: { by: ref/x, at: 2026-06-20T22:53:05Z }\n\
         verified: { by: human:a, at: 2026-06-25T09:00:00Z }\n\
         ---\n\n# Definition\n\nProse.\n",
    );
    tmp.write(
        "citing.md",
        "---\ntype: Metric\ntitle: Citing\ndescription: d\n\
         generated: { by: ref/x, at: 2026-06-20T22:53:05Z }\n\
         verified: { by: human:a, at: 2026-06-25T09:00:00Z }\n\
         ---\n\n# Definition\n\nSee [the cited one](cited.md).\n",
    );
    // No index lists `cited`, but `citing` links to it, so it is not an orphan.
    // `citing` itself is still an orphan (no inbound links, no index).
    let bundle = Bundle::load(tmp.path()).unwrap();
    let report = lint_bundle(&bundle);
    let orphan_ids: Vec<String> = report
        .diagnostics
        .iter()
        .filter(|d| d.message.starts_with("[L15] "))
        .filter_map(|d| d.concept.as_ref().map(ToString::to_string))
        .collect();
    assert!(
        orphan_ids.iter().any(|id| id == "citing"),
        "`citing` should be flagged as an orphan: {orphan_ids:?}"
    );
    assert!(
        !orphan_ids.iter().any(|id| id == "cited"),
        "`cited` has an inbound link, so must not be flagged: {orphan_ids:?}"
    );
}

#[test]
fn l16_index_missing_a_concept_on_disk() {
    let tmp = TempDir::new();
    tmp.write(
        "a.md",
        "---\ntype: Metric\ntitle: A\ndescription: d\n\
         generated: { by: ref/x, at: 2026-06-20T22:53:05Z }\n\
         verified: { by: human:a, at: 2026-06-25T09:00:00Z }\n\
         ---\n\n# Definition\n\nProse.\n",
    );
    tmp.write(
        "b.md",
        "---\ntype: Metric\ntitle: B\ndescription: d\n\
         generated: { by: ref/x, at: 2026-06-20T22:53:05Z }\n\
         verified: { by: human:a, at: 2026-06-25T09:00:00Z }\n\
         ---\n\n# Definition\n\nProse.\n",
    );
    // Index lists only `a`; `b` is on disk but not listed.
    tmp.write("index.md", "# Metric\n\n* [A](a.md)\n");
    let bundle = Bundle::load(tmp.path()).unwrap();
    let report = lint_bundle(&bundle);
    let l16 = messages_for(&report, "L16");
    assert!(
        l16.iter()
            .any(|m| m.contains("missing from index") && m.contains('b')),
        "L16 should flag `b` as missing from the index: {l16:?}"
    );
}

#[test]
fn l16_index_lists_a_concept_not_on_disk() {
    let tmp = TempDir::new();
    tmp.write(
        "a.md",
        "---\ntype: Metric\ntitle: A\ndescription: d\n\
         generated: { by: ref/x, at: 2026-06-20T22:53:05Z }\n\
         verified: { by: human:a, at: 2026-06-25T09:00:00Z }\n\
         ---\n\n# Definition\n\nProse.\n",
    );
    // Index lists `a` and a ghost `b` that has no file.
    tmp.write("index.md", "# Metric\n\n* [A](a.md)\n* [B](b.md)\n");
    let bundle = Bundle::load(tmp.path()).unwrap();
    let report = lint_bundle(&bundle);
    let l16 = messages_for(&report, "L16");
    assert!(
        l16.iter()
            .any(|m| m.contains("listed but not on disk") && m.contains('b')),
        "L16 should flag `b` as listed but not on disk: {l16:?}"
    );
}

#[test]
fn l16_ignores_absolute_links_pointing_elsewhere() {
    let tmp = TempDir::new();
    tmp.write(
        "dir/a.md",
        "---\ntype: Metric\ntitle: A\ndescription: d\n\
         generated: { by: ref/x, at: 2026-06-20T22:53:05Z }\n\
         verified: { by: human:a, at: 2026-06-25T09:00:00Z }\n\
         ---\n\n# Definition\n\nProse.\n",
    );
    // The root index links to `/dir/a.md` (an absolute link). The root dir
    // has no concepts, so the root index must not be flagged as stale.
    tmp.write("index.md", "# Metric\n\n* [A](dir/a.md)\n");
    // A dir index also lists it, so it is not an orphan.
    tmp.write("dir/index.md", "# Metric\n\n* [A](a.md)\n");
    let bundle = Bundle::load(tmp.path()).unwrap();
    let report = lint_bundle(&bundle);
    assert!(
        !has(&report, "L16"),
        "absolute link to /dir/a.md must not flag the root index as stale: {:#?}",
        report.diagnostics
    );
}

#[test]
fn l16_ignores_reserved_files_in_index() {
    let tmp = TempDir::new();
    tmp.write(
        "a.md",
        "---\ntype: Metric\ntitle: A\ndescription: d\n\
         generated: { by: ref/x, at: 2026-06-20T22:53:05Z }\n\
         verified: { by: human:a, at: 2026-06-25T09:00:00Z }\n\
         ---\n\n# Definition\n\nProse.\n",
    );
    tmp.write(
        "log.md",
        "# Update Log\n\n## 2026-06-25\n* **Creation**: Init.\n",
    );
    tmp.write(
        "index.md",
        "# Metric\n\n* [A](a.md)\n\n# Other\n\n* [log](log.md)\n",
    );
    let bundle = Bundle::load(tmp.path()).unwrap();
    let report = lint_bundle(&bundle);
    assert!(
        !has(&report, "L16"),
        "index listing log.md should not trigger L16: {:#?}",
        report.diagnostics
    );
}

#[test]
fn l17_broken_link() {
    let tmp = TempDir::new();
    tmp.write(
        "a.md",
        "---\ntype: Metric\ntitle: A\ndescription: d\n\
         generated: { by: ref/x, at: 2026-06-20T22:53:05Z }\n\
         verified: { by: human:a, at: 2026-06-25T09:00:00Z }\n\
         ---\n\n# Definition\n\nLink to [nonexistent](ghost.md).\n",
    );
    tmp.write("index.md", "# Metric\n\n* [A](a.md)\n");
    let bundle = Bundle::load(tmp.path()).unwrap();
    let report = lint_bundle(&bundle);
    assert!(
        has(&report, "L17"),
        "broken link to ghost.md should trigger L17 warning: {:#?}",
        report.diagnostics
    );
}

#[test]
fn l18_key_order() {
    let tmp = TempDir::new();
    // description before title is non-canonical order
    tmp.write(
        "metric.md",
        "---\ntype: Metric\ndescription: Recognized revenue.\ntitle: Revenue\n\
         generated: { by: ref/x, at: 2026-06-20T22:53:05Z }\n\
         verified: { by: human:a, at: 2026-06-25T09:00:00Z }\n\
         ---\n\n# Definition\n\nProse.\n",
    );
    tmp.write("index.md", "# Metric\n\n* [Revenue](metric.md)\n");
    let bundle = Bundle::load(tmp.path()).unwrap();
    let report = lint_bundle(&bundle);
    assert!(
        has(&report, "L18"),
        "expected L18 for non-canonical key order"
    );
}

#[test]
fn l19_heading_hierarchy() {
    let tmp = TempDir::new();
    // Skips from h1 to h3
    tmp.write(
        "metric.md",
        "---\ntype: Metric\ntitle: Revenue\ndescription: Recognized revenue.\n\
         generated: { by: ref/x, at: 2026-06-20T22:53:05Z }\n\
         verified: { by: human:a, at: 2026-06-25T09:00:00Z }\n\
         ---\n\n# Definition\n\n### Skipped to H3\n\nProse.\n",
    );
    tmp.write("index.md", "# Metric\n\n* [Revenue](metric.md)\n");
    let bundle = Bundle::load(tmp.path()).unwrap();
    let report = lint_bundle(&bundle);
    assert!(has(&report, "L19"), "expected L19 for heading level skip");
}

#[test]
fn l20_empty_heading() {
    let tmp = TempDir::new();
    // Heading with no content
    tmp.write(
        "metric.md",
        "---\ntype: Metric\ntitle: Revenue\ndescription: Recognized revenue.\n\
         generated: { by: ref/x, at: 2026-06-20T22:53:05Z }\n\
         verified: { by: human:a, at: 2026-06-25T09:00:00Z }\n\
         ---\n\n# Definition\n\n## Empty Section\n\n## Next Section\n\nProse.\n",
    );
    tmp.write("index.md", "# Metric\n\n* [Revenue](metric.md)\n");
    let bundle = Bundle::load(tmp.path()).unwrap();
    let report = lint_bundle(&bundle);
    assert!(has(&report, "L20"), "expected L20 for empty heading stub");
}

#[test]
fn l21_unused_source() {
    let tmp = TempDir::new();
    tmp.write(
        "metric.md",
        "---\ntype: Metric\ntitle: Revenue\ndescription: Recognized revenue.\n\
         generated: { by: ref/x, at: 2026-06-20T22:53:05Z }\n\
         verified: { by: human:a, at: 2026-06-25T09:00:00Z }\n\
         sources:\n  - { id: sec-10k, resource: 'https://sec.gov' }\n\
         ---\n\n# Definition\n\nProse with no footnote.\n",
    );
    tmp.write("index.md", "# Metric\n\n* [Revenue](metric.md)\n");
    let bundle = Bundle::load(tmp.path()).unwrap();
    let report = lint_bundle(&bundle);
    assert!(has(&report, "L21"), "expected L21 for uncited source");
}

#[test]
fn l22_circular_derivation() {
    let tmp = TempDir::new();
    tmp.write(
        "a.md",
        "---\ntype: Metric\ntitle: A\ndescription: d\n\
         generated: { by: ref/x, at: 2026-06-20T22:53:05Z }\n\
         verified: { by: human:a, at: 2026-06-25T09:00:00Z }\n\
         sources:\n  - { resource: b.md }\n\
         ---\n\n# Definition\n\nSee [B](b.md).\n",
    );
    tmp.write(
        "b.md",
        "---\ntype: Metric\ntitle: B\ndescription: d\n\
         generated: { by: ref/x, at: 2026-06-20T22:53:05Z }\n\
         verified: { by: human:a, at: 2026-06-25T09:00:00Z }\n\
         sources:\n  - { resource: a.md }\n\
         ---\n\n# Definition\n\nSee [A](a.md).\n",
    );
    tmp.write("index.md", "# Metric\n\n* [A](a.md)\n* [B](b.md)\n");
    let bundle = Bundle::load(tmp.path()).unwrap();
    let report = lint_bundle(&bundle);
    assert!(has(&report, "L22"), "expected L22 for circular derivation");
}

#[test]
fn l23_non_standard_actor() {
    let tmp = TempDir::new();
    tmp.write(
        "metric.md",
        "---\ntype: Metric\ntitle: Revenue\ndescription: d\n\
         generated: { by: 'Alice Baker', at: 2026-06-20T22:53:05Z }\n\
         verified: { by: human:ahormati, at: 2026-06-25T09:00:00Z }\n\
         ---\n\n# Definition\n\nProse.\n",
    );
    tmp.write("index.md", "# Metric\n\n* [Revenue](metric.md)\n");
    let bundle = Bundle::load(tmp.path()).unwrap();
    let report = lint_bundle(&bundle);
    assert!(has(&report, "L23"), "expected L23 for non-standard actor");
}

#[test]
fn l24_future_timestamp() {
    let tmp = TempDir::new();
    tmp.write(
        "metric.md",
        "---\ntype: Metric\ntitle: Revenue\ndescription: d\n\
         generated: { by: ref/x, at: 2099-01-01T00:00:00Z }\n\
         verified: { by: human:a, at: 2026-06-25T09:00:00Z }\n\
         ---\n\n# Definition\n\nProse.\n",
    );
    tmp.write("index.md", "# Metric\n\n* [Revenue](metric.md)\n");
    let bundle = Bundle::load(tmp.path()).unwrap();
    let today = Date::parse("2026-07-01").unwrap();
    let report = lint_bundle_at(&bundle, Some(today));
    assert!(has(&report, "L24"), "expected L24 for future timestamp");
}

#[test]
fn l25_missing_attestation_resource() {
    let tmp = TempDir::new();
    tmp.write(
        "comp.md",
        "---\ntype: Attested Computation\ntitle: Calc\ndescription: d\n\
         generated: { by: ref/x, at: 2026-06-20T22:53:05Z }\n\
         verified: { by: human:a, at: 2026-06-25T09:00:00Z }\n\
         runtime: python\n\
         parameters:\n  - { name: x, type: string }\n\
         executor:\n  resource: non_existent_script.py\n  receipt: [res]\n\
         attester:\n  resource: non_existent_verifier.py\n\
         ---\n\n# Calc\n\n# Computation\n\n```python\nx = 1\n```\n",
    );
    tmp.write("index.md", "# Attested Computation\n\n* [Calc](comp.md)\n");
    let bundle = Bundle::load(tmp.path()).unwrap();
    let report = lint_bundle(&bundle);
    assert!(
        has(&report, "L25"),
        "expected L25 for missing resource on disk"
    );
}

#[test]
fn l26_unlabeled_computation_block() {
    let tmp = TempDir::new();
    tmp.write(
        "comp.md",
        "---\ntype: Attested Computation\ntitle: Calc\ndescription: d\n\
         generated: { by: ref/x, at: 2026-06-20T22:53:05Z }\n\
         verified: { by: human:a, at: 2026-06-25T09:00:00Z }\n\
         runtime: python\n\
         parameters:\n  - { name: x, type: string }\n\
         executor:\n  resource: 'https://example.com/exec'\n  receipt: [res]\n\
         attester:\n  resource: 'https://example.com/attest'\n\
         ---\n\n# Calc\n\n# Computation\n\n```\nx = 1\n```\n",
    );
    tmp.write("index.md", "# Attested Computation\n\n* [Calc](comp.md)\n");
    let bundle = Bundle::load(tmp.path()).unwrap();
    let report = lint_bundle(&bundle);
    assert!(has(&report, "L26"), "expected L26 for unlabeled code block");
}

#[test]
fn l27_duplicate_log_date() {
    let tmp = TempDir::new();
    tmp.write("metric.md", MIN_CONCEPT);
    tmp.write("index.md", "# Metric\n\n* [Revenue](metric.md)\n");
    tmp.write(
        "log.md",
        "# Update Log\n\n## 2026-05-22\n* **Update**: First.\n\n## 2026-05-22\n* **Update**: Duplicate date.\n",
    );
    let bundle = Bundle::load(tmp.path()).unwrap();
    let report = lint_bundle(&bundle);
    assert!(has(&report, "L27"), "expected L27 for duplicate log date");
}

#[test]
fn l28_trailing_and_excess_whitespace() {
    let tmp = TempDir::new();
    // Test case 1: trailing whitespace on line "sometext   \n"
    tmp.write(
        "t1.md",
        "---\ntype: Concept\ntitle: T1\ndescription: d\ngenerated:\n  by: ref/x\n  at: 2026-01-01T00:00:00Z\n---\n\n# T1\n\nsometext   \n",
    );
    // Test case 2: trailing whitespace and trailing whitespace-only lines "sometext     \n   \n   \n     "
    tmp.write(
        "t2.md",
        "---\ntype: Concept\ntitle: T2\ndescription: d\ngenerated:\n  by: ref/x\n  at: 2026-01-01T00:00:00Z\n---\n\n# T2\n\nsometext     \n   \n   \n     ",
    );
    // Test case 3: trailing tabs "sometext\t\t\n"
    tmp.write(
        "t3.md",
        "---\ntype: Concept\ntitle: T3\ndescription: d\ngenerated:\n  by: ref/x\n  at: 2026-01-01T00:00:00Z\n---\n\n# T3\n\nsometext\t\t\n",
    );
    // Test case 4: excess consecutive blank lines in middle of body
    tmp.write(
        "t4.md",
        "---\ntype: Concept\ntitle: T4\ndescription: d\ngenerated:\n  by: ref/x\n  at: 2026-01-01T00:00:00Z\n---\n\n# T4\n\n\n\n\nsometext\n",
    );
    // Test case 5: trailing blank lines at end of body
    tmp.write(
        "t5.md",
        "---\ntype: Concept\ntitle: T5\ndescription: d\ngenerated:\n  by: ref/x\n  at: 2026-01-01T00:00:00Z\n---\n\n# T5\n\nsometext\n\n\n\n",
    );
    tmp.write(
        "index.md",
        "# Concepts\n\n* [T1](t1.md)\n* [T2](t2.md)\n* [T3](t3.md)\n* [T4](t4.md)\n* [T5](t5.md)\n",
    );

    let bundle = Bundle::load(tmp.path()).unwrap();
    let report = lint_bundle(&bundle);
    let l28_diagnostics: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.message.starts_with("[L28]"))
        .collect();
    assert_eq!(
        l28_diagnostics.len(),
        5,
        "Every whitespace variant should trigger L28: {l28_diagnostics:#?}"
    );
}
