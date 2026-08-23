//! Integration tests for `okf diff`: the OKF-semantics bundle diff.

mod common;

use common::TempDir;
use okf::{Bundle, bundle_diff};

/// A minimal conformant concept body.
const CONCEPT: &str = "---\n\
type: Metric\n\
title: Revenue\n\
description: Recognized revenue.\n\
---\n\n\
# Definition\n\n\
Recognized revenue.\n";

fn load(dir: &TempDir) -> Bundle {
    Bundle::load(dir.path()).unwrap()
}

#[test]
fn identical_bundles_produce_no_changes() {
    let a = TempDir::new();
    a.write("metric.md", CONCEPT);
    let b = TempDir::new();
    b.write("metric.md", CONCEPT);

    let diff = bundle_diff(&load(&a), &load(&b));
    assert!(diff.is_empty());
    assert_eq!(diff.to_string(), "no changes\n");
}

#[test]
fn added_and_removed_concepts_are_reported() {
    let a = TempDir::new();
    a.write("old.md", CONCEPT);
    let b = TempDir::new();
    b.write(
        "new.md",
        "---\ntype: Metric\ntitle: Other\n---\n\n# Definition\n\nDifferent body.\n",
    );

    let diff = bundle_diff(&load(&a), &load(&b));
    assert_eq!(diff.removed.len(), 1);
    assert_eq!(diff.removed[0].to_string(), "old");
    assert_eq!(diff.added.len(), 1);
    assert_eq!(diff.added[0].to_string(), "new");
    assert!(diff.renamed.is_empty());
}

#[test]
fn rename_is_detected_by_content_hash() {
    // Same content under a new id: a rename, not an add plus a remove.
    let a = TempDir::new();
    a.write("revenue.md", CONCEPT);
    let b = TempDir::new();
    b.write("net_revenue.md", CONCEPT);

    let diff = bundle_diff(&load(&a), &load(&b));
    assert_eq!(diff.renamed.len(), 1);
    assert_eq!(diff.renamed[0].from.to_string(), "revenue");
    assert_eq!(diff.renamed[0].to.to_string(), "net_revenue");
    assert!(diff.added.is_empty());
    assert!(diff.removed.is_empty());
}

#[test]
fn edit_breaks_rename_detection() {
    // Different bodies means the move is reported as add + remove.
    let a = TempDir::new();
    a.write("revenue.md", CONCEPT);
    let b = TempDir::new();
    b.write(
        "net_revenue.md",
        "---\ntype: Metric\ntitle: Revenue\n---\n\n# Definition\n\nChanged.\n",
    );

    let diff = bundle_diff(&load(&a), &load(&b));
    assert!(diff.renamed.is_empty());
    assert_eq!(diff.added.len(), 1);
    assert_eq!(diff.removed.len(), 1);
}

#[test]
fn same_id_body_changes_are_reported() {
    let a = TempDir::new();
    a.write("metric.md", CONCEPT);
    let b = TempDir::new();
    b.write(
        "metric.md",
        "---\ntype: Metric\ntitle: Revenue\ndescription: Recognized revenue.\n---\n\n# Definition\n\nUpdated revenue definition.\n",
    );

    let diff = bundle_diff(&load(&a), &load(&b));
    assert_eq!(diff.content.len(), 1);
    assert_eq!(diff.content[0].to_string(), "metric");
    assert!(diff.frontmatter.is_empty());
    assert!(diff.to_string().contains("content (1):"));
}

#[test]
fn frontmatter_key_changes_are_reported() {
    let a = TempDir::new();
    a.write(
        "metric.md",
        "---\ntype: Metric\ntitle: Revenue\ndescription: Old.\nstatus: stable\n---\n\n# Definition\n\nx\n",
    );
    let b = TempDir::new();
    b.write(
        "metric.md",
        "---\ntype: Metric\ntitle: Revenue\ndescription: New.\nstatus: deprecated\ntags: [x]\n---\n\n# Definition\n\nx\n",
    );

    let diff = bundle_diff(&load(&a), &load(&b));
    assert_eq!(diff.frontmatter.len(), 1);
    let fc = &diff.frontmatter[0];
    assert_eq!(fc.id.to_string(), "metric");
    assert_eq!(fc.added, vec!["tags".to_string()]);
    assert!(fc.removed.is_empty());
    let changed_keys: Vec<&str> = fc.changed.iter().map(|(k, _, _)| k.as_str()).collect();
    assert!(changed_keys.contains(&"description"));
    assert!(changed_keys.contains(&"status"));
}

#[test]
fn trust_tier_and_status_changes_are_reported() {
    let a = TempDir::new();
    a.write(
        "metric.md",
        "---\ntype: Metric\nstatus: draft\n---\n\n# Definition\n\nx\n",
    );
    let b = TempDir::new();
    b.write(
        "metric.md",
        "---\ntype: Metric\nverified: { by: human:ahormati, at: 2026-06-25T09:00:00Z }\nstatus: stable\n---\n\n# Definition\n\nx\n",
    );

    let diff = bundle_diff(&load(&a), &load(&b));
    assert_eq!(diff.trust.len(), 1);
    let tc = &diff.trust[0];
    assert_eq!(tc.id.to_string(), "metric");
    assert!(tc.tier.is_some());
    assert!(tc.status.is_some());
}

#[test]
fn mended_and_broken_links_are_reported() {
    let a = TempDir::new();
    a.write(
        "metric.md",
        "---\ntype: Metric\n---\n\n# Definition\n\nSee [old](missing.md).\n",
    );
    let b = TempDir::new();
    b.write(
        "metric.md",
        "---\ntype: Metric\n---\n\n# Definition\n\nSee [now](other.md).\n",
    );
    b.write("other.md", CONCEPT);

    let diff = bundle_diff(&load(&a), &load(&b));
    // `missing.md` was broken in a and is gone from b -> mended.
    assert!(diff.mended_links.iter().any(|(_, t)| t == "missing.md"));
    // `other.md` is broken in b (it links to a concept that exists, so actually
    // it should be mended). Recompute: b's metric links to `other.md` which
    // exists in b, so it is not broken. Instead a's `missing.md` is mended.
    assert!(diff.broken_links.is_empty());
}

#[test]
fn newly_broken_link_is_reported() {
    let a = TempDir::new();
    a.write(
        "metric.md",
        "---\ntype: Metric\n---\n\n# Definition\n\nSee [x](other.md).\n",
    );
    a.write("other.md", CONCEPT);

    let b = TempDir::new();
    b.write(
        "metric.md",
        "---\ntype: Metric\n---\n\n# Definition\n\nSee [x](other.md).\n",
    );
    // `other.md` removed in b, so the link is now broken.

    let diff = bundle_diff(&load(&a), &load(&b));
    assert!(diff.broken_links.iter().any(|(_, t)| t == "other.md"));
    assert!(diff.mended_links.is_empty());
}

#[test]
fn valid_link_additions_removals_and_retargeting_are_reported() {
    let a = TempDir::new();
    a.write(
        "source.md",
        "---\ntype: Metric\n---\n\nSee [old](old.md).\n",
    );
    a.write(
        "removed.md",
        "---\ntype: Metric\n---\n\nSee [old](old.md).\n",
    );
    a.write("added.md", "---\ntype: Metric\n---\n\nNo links.\n");
    a.write("old.md", CONCEPT);
    a.write("new.md", CONCEPT);

    let b = TempDir::new();
    b.write(
        "source.md",
        "---\ntype: Metric\n---\n\nSee [new](new.md).\n",
    );
    b.write("removed.md", "---\ntype: Metric\n---\n\nNo links.\n");
    b.write("added.md", "---\ntype: Metric\n---\n\nSee [new](new.md).\n");
    b.write("old.md", CONCEPT);
    b.write("new.md", CONCEPT);

    let diff = bundle_diff(&load(&a), &load(&b));
    assert!(
        diff.added_links
            .iter()
            .any(|(source, target)| source.name() == "source" && target.name() == "new")
    );
    assert!(
        diff.added_links
            .iter()
            .any(|(source, target)| source.name() == "added" && target.name() == "new")
    );
    assert!(
        diff.removed_links
            .iter()
            .any(|(source, target)| source.name() == "source" && target.name() == "old")
    );
    assert!(
        diff.removed_links
            .iter()
            .any(|(source, target)| { source.name() == "removed" && target.name() == "old" })
    );
    assert!(diff.mended_links.is_empty());
    assert!(diff.broken_links.is_empty());

    let out = diff.to_string();
    assert!(out.contains("added links (2):"), "{out}");
    assert!(out.contains("removed links (2):"), "{out}");
}

#[test]
fn display_lists_every_section() {
    let a = TempDir::new();
    a.write("old.md", CONCEPT);
    a.write(
        "metric.md",
        "---\ntype: Metric\nstatus: stable\n---\n\n# Definition\n\nx\n",
    );

    let b = TempDir::new();
    b.write(
        "metric.md",
        "---\ntype: Metric\nstatus: deprecated\n---\n\n# Definition\n\nx\n",
    );
    b.write(
        "new.md",
        "---\ntype: Metric\ntitle: Other\n---\n\n# Definition\n\nDifferent body.\n",
    );

    let diff = bundle_diff(&load(&a), &load(&b));
    let out = diff.to_string();
    assert!(out.contains("added (1):"), "{out}");
    assert!(out.contains("removed (1):"), "{out}");
    assert!(out.contains("frontmatter (1):"), "{out}");
    assert!(out.contains("trust (1):"), "{out}");
}
