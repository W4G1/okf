//! Integration tests for `okf lint`.
//!
//! Each test pins one rule against a minimal fixture, so a regression points
//! at the rule that broke. The fixtures are deliberately tiny and independent
//! of the spec's Appendix A bundle, which `conformance.rs` already covers.

mod common;

use common::TempDir;
use okf_core::Bundle;
use okf_validator::{Severity, lint_bundle};

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
verified: { by: human:walter, at: 2026-06-25T09:00:00Z }\n\
---\n\n\
# Definition\n\n\
Recognized revenue.\n";

/// Writes `MIN_CONCEPT` to `metric.md` and loads the bundle.
fn minimal_bundle() -> (TempDir, Bundle) {
    let tmp = TempDir::new();
    tmp.write("metric.md", MIN_CONCEPT);
    // A root index.md keeps the concept from being flagged as an orphan
    // (L9), so the rule under test is the only finding.
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
fn l1_no_top_heading() {
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
    assert!(has(&lint_bundle(&bundle), "L1"));
}

#[test]
fn l2_key_order() {
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
        has(&report, "L2"),
        "expected L2 for non-canonical key order"
    );
}

#[test]
fn l3_heading_hierarchy() {
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
    assert!(has(&report, "L3"), "expected L3 for heading level skip");
}

#[test]
fn l4_empty_heading() {
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
    assert!(has(&report, "L4"), "expected L4 for empty heading stub");
}

#[test]
fn l5_unused_source() {
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
    assert!(has(&report, "L5"), "expected L5 for uncited source");
    let diag = report
        .diagnostics
        .iter()
        .find(|d| d.message.starts_with("[L5] "))
        .expect("L5 diagnostic");
    assert_eq!(diag.severity, Severity::Warning, "L5 must be a warning");
}

#[test]
fn l6_non_standard_actor() {
    let tmp = TempDir::new();
    tmp.write(
        "metric.md",
        "---\ntype: Metric\ntitle: Revenue\ndescription: d\n\
         generated: { by: 'Alice Baker', at: 2026-06-20T22:53:05Z }\n\
         verified: { by: human:walter, at: 2026-06-25T09:00:00Z }\n\
         ---\n\n# Definition\n\nProse.\n",
    );
    tmp.write("index.md", "# Metric\n\n* [Revenue](metric.md)\n");
    let bundle = Bundle::load(tmp.path()).unwrap();
    let report = lint_bundle(&bundle);
    assert!(has(&report, "L6"), "expected L6 for non-standard actor");
}

#[test]
fn l7_unlabeled_computation_block() {
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
    assert!(has(&report, "L7"), "expected L7 for unlabeled code block");
}

#[test]
fn l8_trailing_and_excess_whitespace() {
    let tmp = TempDir::new();
    tmp.write(
        "t1.md",
        "---\ntype: Concept\ntitle: T1\ndescription: d\ngenerated:\n  by: ref/x\n  at: 2026-01-01T00:00:00Z\n---\n\n# T1\n\nsometext   \n",
    );
    tmp.write("index.md", "# Concept\n\n* [T1](t1.md)\n");
    let bundle = Bundle::load(tmp.path()).unwrap();
    let report = lint_bundle(&bundle);
    assert!(has(&report, "L8"));
}

#[test]
fn l9_orphan_when_not_linked_or_indexed() {
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
    assert!(has(&lint_bundle(&bundle), "L9"));
}

#[test]
fn l9_rescinded_by_being_listed_in_an_index() {
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
    assert!(!has(&lint_bundle(&bundle), "L9"));
}

#[test]
fn l9_rescinded_by_an_inbound_link() {
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
    let bundle = Bundle::load(tmp.path()).unwrap();
    let report = lint_bundle(&bundle);
    let orphan_ids: Vec<String> = report
        .diagnostics
        .iter()
        .filter(|d| d.message.starts_with("[L9] "))
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
fn l10_self_link() {
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
    assert!(has(&report, "L10"));
    let is_info = report
        .diagnostics
        .iter()
        .any(|d| d.severity == Severity::Info && d.message.starts_with("[L10] "));
    assert!(is_info, "L10 must be info");
}

#[test]
fn l11_unverified_is_info() {
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
    assert!(has(&report, "L11"));
    let is_info = report
        .diagnostics
        .iter()
        .any(|d| d.severity == Severity::Info && d.message.starts_with("[L11] "));
    assert!(is_info, "L11 must be info, not warning");
}

#[test]
fn l12_draft_status_is_info() {
    let tmp = TempDir::new();
    tmp.write(
        "metric.md",
        "---\ntype: Metric\ntitle: Revenue\ndescription: d\n\
         status: draft\n\
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
fn l13_unquoted_okf_version() {
    let tmp = TempDir::new();
    tmp.write("metric.md", MIN_CONCEPT);
    tmp.write(
        "index.md",
        "---\nokf_version: 0.2\n---\n\n# Metric\n\n* [Revenue](metric.md)\n",
    );
    let bundle = Bundle::load(tmp.path()).unwrap();
    let report = lint_bundle(&bundle);
    assert!(has(&report, "L13"), "expected L13 for unquoted okf_version");
    let diag = report
        .diagnostics
        .iter()
        .find(|d| d.message.starts_with("[L13] "))
        .expect("L13 diagnostic");
    assert_eq!(diag.severity, Severity::Warning, "L13 must be a warning");
    assert!(diag.fixable, "L13 must be fixable");
}
