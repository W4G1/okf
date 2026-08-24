//! Tests for OKF refactoring operations: move/rename, remove, split, and merge.

mod common;

use common::TempDir;
use okf_core::Bundle;
use okf_core::concept_id::ConceptId;
use okf_core::refactor::{
    MergeOptions, MoveOptions, RefactorError, RemoveOptions, SplitOptions, merge_concepts,
    move_concept, remove_concept, split_concept,
};

#[test]
fn test_move_concept_rewrites_incoming_and_rebases_outgoing() {
    let tmp = TempDir::new();
    tmp.write("index.md", "---\nokf_version: \"0.2\"\n---\n\n# Bundle\n");
    tmp.write(
        "auth/token.md",
        "---\ntype: Concept\ntitle: Auth Token\n---\n\n# Auth Token\n\nSee [User Profile](../users/profile.md) for user details.\n",
    );
    tmp.write(
        "users/profile.md",
        "---\ntype: Concept\ntitle: User Profile\n---\n\n# User Profile\n\nUses [Auth Token](../auth/token.md) for authentication.\n",
    );
    tmp.write(
        "overview.md",
        "---\ntype: Concept\ntitle: Overview\n---\n\n# Overview\n\nCheck [Auth Token](auth/token.md#expiration) and [/auth/token.md](/auth/token.md).\n",
    );

    let bundle = Bundle::load(tmp.path()).unwrap();
    let from = ConceptId::parse("auth/token").unwrap();
    let to = ConceptId::parse("security/jwt/token").unwrap();

    let opts = MoveOptions::default();
    let report = move_concept(&bundle, &from, &to, &opts).unwrap();

    assert_eq!(report.rewritten_incoming_links, 3);
    assert_eq!(report.rebased_outgoing_links, 1);
    assert!(!tmp.path().join("auth/token.md").exists());
    assert!(tmp.path().join("security/jwt/token.md").exists());

    // Check moved file outgoing link
    let moved_content = tmp.read("security/jwt/token.md");
    assert!(
        moved_content.contains("[User Profile](../../users/profile.md)"),
        "Moved content was: {moved_content}"
    );

    // Check users/profile incoming link
    let users_content = tmp.read("users/profile.md");
    assert!(
        users_content.contains("[Auth Token](../security/jwt/token.md)"),
        "Users content was: {users_content}"
    );

    // Check overview incoming links (both relative with anchor and absolute)
    let overview_content = tmp.read("overview.md");
    assert!(
        overview_content.contains("[Auth Token](security/jwt/token.md#expiration)"),
        "Overview content was: {overview_content}"
    );
    assert!(
        overview_content.contains("[/auth/token.md](/security/jwt/token.md)"),
        "Overview content was: {overview_content}"
    );

    // Check log.md update
    let log_content = tmp.read("log.md");
    assert!(log_content.contains("Renamed concept `auth/token` to `security/jwt/token`"));
}

#[test]
fn test_move_concept_rebases_frontmatter_resources() {
    let tmp = TempDir::new();
    tmp.write("index.md", "---\nokf_version: \"0.2\"\n---\n\n# Bundle\n");
    tmp.write(
        "metrics/calc/revenue.md",
        "---\ntype: Attested Computation\ntitle: Revenue\nruntime: python\ncomputation: ../scripts/rev.py\nexecutor:\n  resource: ../skills/run.md\n  receipt: [result]\nattester:\n  resource: ../attesters/verify.py\nsources:\n  - id: ga4\n    resource: ../data/schema.json\n---\n\n# Revenue\n\nComputed via rev.py.\n",
    );

    let bundle = Bundle::load(tmp.path()).unwrap();
    let from = ConceptId::parse("metrics/calc/revenue").unwrap();
    let to = ConceptId::parse("finance/revenue").unwrap();

    let opts = MoveOptions::default();
    let report = move_concept(&bundle, &from, &to, &opts).unwrap();

    assert_eq!(report.rebased_frontmatter_paths, 4);

    let moved_content = tmp.read("finance/revenue.md");
    assert!(moved_content.contains("computation: ../metrics/scripts/rev.py"));
    assert!(moved_content.contains("resource: ../metrics/skills/run.md"));
    assert!(moved_content.contains("resource: ../metrics/attesters/verify.py"));
    assert!(moved_content.contains("resource: ../metrics/data/schema.json"));
}

#[test]
fn test_move_concept_dry_run() {
    let tmp = TempDir::new();
    tmp.write("index.md", "---\nokf_version: \"0.2\"\n---\n\n# Bundle\n");
    tmp.write("auth.md", "---\ntype: Concept\n---\n\n# Auth\n");
    tmp.write(
        "user.md",
        "---\ntype: Concept\n---\n\n# User\n\n[Auth](auth.md)\n",
    );

    let bundle = Bundle::load(tmp.path()).unwrap();
    let from = ConceptId::parse("auth").unwrap();
    let to = ConceptId::parse("security/auth").unwrap();

    let opts = MoveOptions {
        dry_run: true,
        ..Default::default()
    };
    let report = move_concept(&bundle, &from, &to, &opts).unwrap();

    assert!(report.dry_run);
    assert_eq!(report.rewritten_incoming_links, 1);
    // Source file must still exist and target must not exist
    assert!(tmp.path().join("auth.md").exists());
    assert!(!tmp.path().join("security/auth.md").exists());
}

#[test]
fn test_remove_concept_protection_and_redirect() {
    let tmp = TempDir::new();
    tmp.write("index.md", "---\nokf_version: \"0.2\"\n---\n\n# Bundle\n");
    tmp.write(
        "legacy.md",
        "---\ntype: Concept\ntitle: Legacy\n---\n\n# Legacy\n",
    );
    tmp.write(
        "modern.md",
        "---\ntype: Concept\ntitle: Modern\n---\n\n# Modern\n",
    );
    tmp.write(
        "user.md",
        "---\ntype: Concept\ntitle: User\n---\n\n# User\n\nSee [Legacy](legacy.md).\n",
    );

    let bundle = Bundle::load(tmp.path()).unwrap();
    let target = ConceptId::parse("legacy").unwrap();
    let redirect = ConceptId::parse("modern").unwrap();

    // 1. Without force, redirect, or unlink -> fails
    let err = remove_concept(&bundle, &target, &RemoveOptions::default()).unwrap_err();
    assert!(matches!(err, RefactorError::HasInboundLinks { .. }));

    // 2. With redirect_to
    let report = remove_concept(
        &bundle,
        &target,
        &RemoveOptions {
            redirect_to: Some(redirect),
            ..Default::default()
        },
    )
    .unwrap();

    assert_eq!(report.redirected_count, 1);
    assert!(!tmp.path().join("legacy.md").exists());

    let user_content = tmp.read("user.md");
    assert!(user_content.contains("[Legacy](modern.md)"));
}

#[test]
fn test_remove_concept_unlink() {
    let tmp = TempDir::new();
    tmp.write("index.md", "---\nokf_version: \"0.2\"\n---\n\n# Bundle\n");
    tmp.write("old.md", "---\ntype: Concept\n---\n\n# Old\n");
    tmp.write(
        "user.md",
        "---\ntype: Concept\n---\n\n# User\n\nSee [Old Docs](old.md) for details.\n",
    );

    let bundle = Bundle::load(tmp.path()).unwrap();
    let target = ConceptId::parse("old").unwrap();

    let report = remove_concept(
        &bundle,
        &target,
        &RemoveOptions {
            unlink: true,
            ..Default::default()
        },
    )
    .unwrap();

    assert_eq!(report.unlinked_count, 1);
    assert!(!tmp.path().join("old.md").exists());

    let user_content = tmp.read("user.md");
    assert_eq!(
        user_content.trim(),
        "---\ntype: Concept\n---\n\n# User\n\nSee Old Docs for details."
    );
}

#[test]
fn test_split_concept_extracts_section_and_migrates_footnotes() {
    let tmp = TempDir::new();
    tmp.write("index.md", "---\nokf_version: \"0.2\"\n---\n\n# Bundle\n");
    let source_body = "\
---
type: Concept
title: Payments
sources:
  - id: pci-dss
    resource: https://pcisecuritystandards.org
    title: PCI-DSS Spec
  - id: stripe-docs
    resource: https://stripe.com/docs
    title: Stripe Documentation
---

# Payments

Payments are processed via Stripe.[^stripe-docs]

## Security Policy

All cardholder data must comply with PCI-DSS.[^pci-dss]
Retention is limited to tokenized cards.

## Reconciliation

Daily settlement reports are verified.
";
    tmp.write("billing/payments.md", source_body);

    let bundle = Bundle::load(tmp.path()).unwrap();
    let source = ConceptId::parse("billing/payments").unwrap();
    let target = ConceptId::parse("security/pci").unwrap();

    let report = split_concept(
        &bundle,
        &source,
        &target,
        &SplitOptions {
            section: "Security Policy".to_string(),
            title: Some("PCI Compliance".to_string()),
            ..Default::default()
        },
    )
    .unwrap();

    assert_eq!(report.target_title, "PCI Compliance");
    assert!(tmp.path().join("security/pci.md").exists());

    let target_content = tmp.read("security/pci.md");
    assert!(target_content.contains("title: PCI Compliance"));
    assert!(target_content.contains("resource: https://pcisecuritystandards.org"));
    assert!(target_content.contains("# PCI Compliance"));
    assert!(target_content.contains("All cardholder data must comply with PCI-DSS.[^pci-dss]"));

    let source_content = tmp.read("billing/payments.md");
    assert!(
        source_content
            .contains("## Security Policy\n\nSee [PCI Compliance](../security/pci.md).\n")
    );
    assert!(source_content.contains("## Reconciliation"));
}

#[test]
fn test_merge_concepts_consolidates_and_redirects() {
    let tmp = TempDir::new();
    tmp.write("index.md", "---\nokf_version: \"0.2\"\n---\n\n# Bundle\n");
    tmp.write(
        "billing/invoices.md",
        "---\ntype: Concept\ntitle: Invoices\nsources:\n  - id: inv-spec\n    resource: https://example.com/inv\nverified:\n  - by: human:alice\n    at: 2026-06-01T00:00:00Z\n---\n\n# Invoices\n\nInvoice details.[^inv-spec]\n",
    );
    tmp.write(
        "finance/invoicing.md",
        "---\ntype: Concept\ntitle: Invoicing\nsources:\n  - id: fin-spec\n    resource: https://example.com/fin\n---\n\n# Invoicing\n\nFinance rules.[^fin-spec]\n",
    );
    tmp.write(
        "overview.md",
        "---\ntype: Concept\ntitle: Overview\n---\n\n# Overview\n\nRead [Invoices](billing/invoices.md).\n",
    );

    let bundle = Bundle::load(tmp.path()).unwrap();
    let source = ConceptId::parse("billing/invoices").unwrap();
    let target = ConceptId::parse("finance/invoicing").unwrap();

    let report = merge_concepts(&bundle, &source, &target, &MergeOptions::default()).unwrap();

    assert_eq!(report.rewritten_links_count, 1);
    assert_eq!(report.merged_sources_count, 1);
    assert!(!tmp.path().join("billing/invoices.md").exists());

    let target_content = tmp.read("finance/invoicing.md");
    assert!(target_content.contains("Finance rules.[^fin-spec]"));
    assert!(target_content.contains("## Invoices"));
    assert!(target_content.contains("Invoice details.[^inv-spec]"));
    assert!(target_content.contains("resource: https://example.com/inv"));
    assert!(target_content.contains("by: human:alice"));

    let overview_content = tmp.read("overview.md");
    assert!(overview_content.contains("[Invoices](finance/invoicing.md)"));
}

#[test]
fn test_compute_relative_path_and_rebase_edge_cases() {
    use okf_core::refactor::{compute_relative_path, rebase_relative_path};

    // Deep to deep with common prefix
    let a = ConceptId::parse("a/b/c/d").unwrap();
    let b = ConceptId::parse("a/b/e/f").unwrap();
    assert_eq!(compute_relative_path(&a, &b), "../e/f.md");

    // Parent to nested child
    let parent = ConceptId::parse("a").unwrap();
    let child = ConceptId::parse("a/b/c").unwrap();
    assert_eq!(compute_relative_path(&parent, &child), "a/b/c.md");

    // Nested child to parent
    assert_eq!(compute_relative_path(&child, &parent), "../../a.md");

    // Rebase relative path edge cases
    let old_dir = vec!["a".to_string(), "b".to_string(), "c".to_string()];
    let new_dir = vec!["x".to_string(), "y".to_string()];

    // In-document anchor should remain unchanged
    assert_eq!(
        rebase_relative_path(&old_dir, &new_dir, "#my-anchor"),
        "#my-anchor"
    );

    // External URI should remain unchanged
    assert_eq!(
        rebase_relative_path(&old_dir, &new_dir, "https://example.com/api#doc"),
        "https://example.com/api#doc"
    );

    // Absolute path should remain unchanged
    assert_eq!(
        rebase_relative_path(&old_dir, &new_dir, "/data/schema.json"),
        "/data/schema.json"
    );

    // Deep relative file path
    assert_eq!(
        rebase_relative_path(&old_dir, &new_dir, "../../data/schema.json"),
        "../../a/data/schema.json"
    );
}

#[test]
fn test_merge_concepts_with_colliding_sources_and_footnotes() {
    let tmp = TempDir::new();
    tmp.write("index.md", "---\nokf_version: \"0.2\"\n---\n\n# Bundle\n");
    tmp.write(
        "source_doc.md",
        "---\ntype: Concept\ntitle: Source Doc\nsources:\n  - id: spec\n    resource: https://source.example.com/spec\n---\n\n# Source Doc\n\nSource claim.[^spec]\n",
    );
    tmp.write(
        "target_doc.md",
        "---\ntype: Concept\ntitle: Target Doc\nsources:\n  - id: spec\n    resource: https://target.example.com/different_spec\n---\n\n# Target Doc\n\nTarget claim.[^spec]\n",
    );

    let bundle = Bundle::load(tmp.path()).unwrap();
    let source = ConceptId::parse("source_doc").unwrap();
    let target = ConceptId::parse("target_doc").unwrap();

    let report = merge_concepts(&bundle, &source, &target, &MergeOptions::default()).unwrap();

    assert_eq!(report.merged_sources_count, 1);

    let target_content = tmp.read("target_doc.md");
    // Original target source kept
    assert!(target_content.contains("resource: https://target.example.com/different_spec"));
    // Source source remapped to source_doc_spec
    assert!(target_content.contains("id: source_doc_spec"));
    assert!(target_content.contains("resource: https://source.example.com/spec"));
    // Source body footnote reference remapped
    assert!(target_content.contains("Source claim.[^source_doc_spec]"));
    // Target body footnote reference unchanged
    assert!(target_content.contains("Target claim.[^spec]"));
}

#[test]
fn test_rename_section_with_deep_heading_and_slug() {
    use okf_core::refactor::{RenameSectionOptions, rename_section};

    let tmp = TempDir::new();
    tmp.write("index.md", "---\nokf_version: \"0.2\"\n---\n\n# Bundle\n");
    tmp.write(
        "auth.md",
        "---\ntype: Concept\ntitle: Auth\n---\n\n# Auth\n\nSee [OAuth Info](#oauth-20--token-auth).\n\n### OAuth 2.0 & Token Auth!\n\nDetails.\n",
    );
    tmp.write(
        "users.md",
        "---\ntype: Concept\ntitle: Users\n---\n\n# Users\n\nRead [Auth Tokens](auth.md#oauth-20--token-auth).\n",
    );

    let bundle = Bundle::load(tmp.path()).unwrap();
    let concept = ConceptId::parse("auth").unwrap();

    let report = rename_section(
        &bundle,
        &concept,
        "OAuth 2.0 & Token Auth!",
        "Modern OAuth",
        &RenameSectionOptions::default(),
    )
    .unwrap();

    assert_eq!(report.internal_links_updated, 1);
    assert_eq!(report.external_links_updated, 1);

    let auth_content = tmp.read("auth.md");
    assert!(auth_content.contains("### Modern OAuth"));
    assert!(auth_content.contains("[OAuth Info](#modern-oauth)"));

    let users_content = tmp.read("users.md");
    assert!(users_content.contains("[Auth Tokens](auth.md#modern-oauth)"));
}
