//! Integration tests for `okf lint` (L1..L16).
//!
//! Each test pins one rule against a minimal fixture, so a regression points
//! at the rule that broke. The fixtures are deliberately tiny and independent
//! of the spec's Appendix A bundle, which `conformance.rs` already covers.

mod common;

use common::TempDir;
use okf::{lint_bundle, lint_bundle_at, Bundle, Date, Severity};

/// Returns the messages of every finding with the given rule code (without
/// the `[code] ` prefix).
fn messages_for<'a>(report: &'a okf::validate::Report, code: &'a str) -> Vec<&'a str> {
    let prefix = format!("[{code}] ");
    report
        .diagnostics
        .iter()
        .filter_map(move |d| d.message.strip_prefix(&prefix))
        .collect()
}

fn has(report: &okf::validate::Report, code: &str) -> bool {
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
         stale_after: 2026-06-15\n\
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
        l11.iter().any(|m| m.contains("stale since 2026-06-15")),
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
            .any(|m| m.contains("missing from index") && m.contains("b")),
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
            .any(|m| m.contains("listed but not on disk") && m.contains("b")),
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
