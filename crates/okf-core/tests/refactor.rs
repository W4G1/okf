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
    tmp.write("index.md", "---\nokf_version: \"0.3\"\n---\n\n# Bundle\n");
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
    tmp.write("index.md", "---\nokf_version: \"0.3\"\n---\n\n# Bundle\n");
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
    tmp.write("index.md", "---\nokf_version: \"0.3\"\n---\n\n# Bundle\n");
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
    tmp.write("index.md", "---\nokf_version: \"0.3\"\n---\n\n# Bundle\n");
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
    tmp.write("index.md", "---\nokf_version: \"0.3\"\n---\n\n# Bundle\n");
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
    tmp.write("index.md", "---\nokf_version: \"0.3\"\n---\n\n# Bundle\n");
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
    tmp.write("index.md", "---\nokf_version: \"0.3\"\n---\n\n# Bundle\n");
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
    tmp.write("index.md", "---\nokf_version: \"0.3\"\n---\n\n# Bundle\n");
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
    tmp.write("index.md", "---\nokf_version: \"0.3\"\n---\n\n# Bundle\n");
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

#[test]
fn test_move_concept_keeps_path_fields_pointing_at_the_same_files() {
    // A bundle that declares nothing, with one path written the way OKF v0.2
    // wrote it: from the bundle root, without a leading `/`.
    let tmp = TempDir::new();
    tmp.write("index.md", "# Bundle\n");
    tmp.write("references/skills/run.md", "# Run\n\nInstructions.\n");
    tmp.write("references/attesters/check.py", "print('ok')\n");
    tmp.write("policies/revenue.md", "---\ntype: Policy\n---\n\nPolicy.\n");
    tmp.write("computations/lib/revenue.sql", "SELECT 1\n");
    tmp.write(
        "computations/revenue.md",
        "---\n\
         type: Attested Computation\n\
         runtime: bigquery\n\
         resource: acme.finance.revenue\n\
         computation: lib/revenue.sql\n\
         executor:\n\
         \x20 resource: references/skills/run.md\n\
         \x20 receipt: [job_id]\n\
         attester: { resource: /references/attesters/check.py, x-acme-timeout: 30 }\n\
         sources:\n\
         \x20 - { id: policy, resource: ../policies/revenue.md, x-acme-weight: 2 }\n\
         \x20 - { id: scope, resource: all queries in project X }\n\
         \x20 - { id: web, resource: https://example.com/policy }\n\
         ---\n\n# Revenue\n",
    );

    let bundle = Bundle::load(tmp.path()).unwrap();
    let from = ConceptId::parse("computations/revenue").unwrap();
    let to = ConceptId::parse("finance/metrics/revenue").unwrap();
    let report = move_concept(&bundle, &from, &to, &MoveOptions::default()).unwrap();
    assert_eq!(report.rebased_frontmatter_paths, 3);

    let moved = tmp.read("finance/metrics/revenue.md");
    // A file-relative path is rebased, to a file or to a concept.
    assert!(
        moved.contains("computation: ../../computations/lib/revenue.sql\n"),
        "{moved}"
    );
    assert!(
        moved.contains("resource: ../../policies/revenue.md"),
        "{moved}"
    );
    // A path that only resolved from the bundle root gains its leading `/`,
    // which makes it right wherever the concept lives.
    assert!(
        moved.contains("  resource: /references/skills/run.md\n"),
        "{moved}"
    );
    // A rooted path, a URL, a scope descriptor, and an opaque `resource`
    // identifier do not depend on the location and are left as written.
    assert!(
        moved.contains(
            "attester: { resource: /references/attesters/check.py, x-acme-timeout: 30 }\n"
        ),
        "{moved}"
    );
    assert!(
        moved.contains("resource: acme.finance.revenue\n"),
        "{moved}"
    );
    assert!(
        moved.contains("resource: all queries in project X"),
        "{moved}"
    );
    assert!(
        moved.contains("resource: https://example.com/policy"),
        "{moved}"
    );
    // Keys this crate does not know travel with the entry.
    assert!(moved.contains("x-acme-weight: 2"), "{moved}");

    // Everything still resolves from the new location.
    let bundle = Bundle::load(tmp.path()).unwrap();
    assert!(bundle.legacy_path_fields().is_empty());
    assert_eq!(
        bundle.derived_from(&to),
        vec![&ConceptId::parse("policies/revenue").unwrap()]
    );
    for raw in [
        "../../computations/lib/revenue.sql",
        "/references/skills/run.md",
        "/references/attesters/check.py",
    ] {
        assert!(bundle.resolve_path_field(&to, raw).is_some(), "{raw}");
    }
}

#[test]
fn test_move_concept_rewrites_path_fields_that_name_it() {
    let tmp = TempDir::new();
    tmp.write("index.md", "---\nokf_version: \"0.3\"\n---\n\n# Bundle\n");
    tmp.write("metrics/margin.md", "---\ntype: Metric\n---\n\n# Margin\n");
    tmp.write(
        "metrics/margin-legacy.md",
        "---\ntype: Metric\nstatus: deprecated\nsuperseded_by: /metrics/margin.md\n---\n\n# Legacy\n",
    );
    tmp.write(
        "reports/q1.md",
        "---\ntype: Report\nsources:\n  - { id: m, resource: ../metrics/margin.md#definition }\n---\n\n\
         Per the [margin][m] metric and the [glossary].\n\n\
         [m]: ../metrics/margin.md \"Margin\"\n[glossary]: ../glossary.md\n",
    );

    let bundle = Bundle::load(tmp.path()).unwrap();
    let from = ConceptId::parse("metrics/margin").unwrap();
    let to = ConceptId::parse("finance/gross-margin").unwrap();
    // A path field is an edge of the bundle graph, like a link.
    assert_eq!(bundle.referenced_by(&from).len(), 2);
    move_concept(&bundle, &from, &to, &MoveOptions::default()).unwrap();

    // A rooted value stays rooted, a relative one stays relative, and the
    // fragment survives.
    let legacy = tmp.read("metrics/margin-legacy.md");
    assert!(
        legacy.contains("superseded_by: /finance/gross-margin.md\n"),
        "{legacy}"
    );
    let report = tmp.read("reports/q1.md");
    assert!(
        report.contains("resource: ../finance/gross-margin.md#definition"),
        "{report}"
    );
    // A reference-style link is rewritten through its definition, title kept.
    assert!(
        report.contains("[m]: ../finance/gross-margin.md \"Margin\"\n"),
        "{report}"
    );
    assert!(report.contains("Per the [margin][m] metric"), "{report}");
    assert!(report.contains("[glossary]: ../glossary.md\n"), "{report}");

    let bundle = Bundle::load(tmp.path()).unwrap();
    assert_eq!(
        bundle.superseded_by(&ConceptId::parse("metrics/margin-legacy").unwrap()),
        Some(&to)
    );
    assert_eq!(bundle.backlinks(&to).len(), 1);
}

#[test]
fn test_move_concept_can_leave_a_deprecated_stub() {
    let tmp = TempDir::new();
    tmp.write("index.md", "---\nokf_version: \"0.3\"\n---\n\n# Bundle\n");
    tmp.write(
        "metrics/mrr.md",
        "---\ntype: Metric\ntitle: Monthly recurring revenue\ntags: [finance]\n---\n\n# MRR\n\nBody.\n",
    );
    tmp.write(
        "overview.md",
        "---\ntype: Concept\n---\n\nSee [MRR](metrics/mrr.md).\n",
    );

    let bundle = Bundle::load(tmp.path()).unwrap();
    let from = ConceptId::parse("metrics/mrr").unwrap();
    let to = ConceptId::parse("finance/mrr").unwrap();
    let report = move_concept(
        &bundle,
        &from,
        &to,
        &MoveOptions {
            leave_stub: true,
            author: Some("human:alice".to_string()),
            ..MoveOptions::default()
        },
    )
    .unwrap();
    assert!(report.left_stub);

    // The concept moved, and known links follow it.
    assert!(tmp.read("finance/mrr.md").contains("# MRR\n\nBody."));
    assert!(tmp.read("overview.md").contains("[MRR](finance/mrr.md)"));

    // The old path keeps a short deprecated concept that names the new one,
    // so a stored reference to it still leads somewhere.
    let stub = tmp.read("metrics/mrr.md");
    assert!(stub.contains("status: deprecated\n"), "{stub}");
    assert!(stub.contains("superseded_by: /finance/mrr.md\n"), "{stub}");
    assert!(
        stub.contains("title: Monthly recurring revenue\n"),
        "{stub}"
    );
    assert!(
        stub.contains("[Monthly recurring revenue](../finance/mrr.md)"),
        "{stub}"
    );

    let bundle = Bundle::load(tmp.path()).unwrap();
    assert_eq!(bundle.superseded_by(&from), Some(&to));
    assert_eq!(bundle.current_replacement(&from).unwrap().id, to);
    assert!(!bundle.get(&from).unwrap().is_current());
}

#[test]
fn test_remove_concept_counts_path_field_references() {
    let tmp = TempDir::new();
    tmp.write("index.md", "---\nokf_version: \"0.3\"\n---\n\n# Bundle\n");
    tmp.write("a.md", "---\ntype: Metric\n---\n\n# A\n");
    tmp.write("b.md", "---\ntype: Metric\n---\n\n# B\n");
    tmp.write(
        "old.md",
        "---\ntype: Metric\nstatus: deprecated\nsuperseded_by: a.md\n---\n\n# Old\n",
    );

    // Nothing links to `a`, but `old` names it in a path field.
    let bundle = Bundle::load(tmp.path()).unwrap();
    let a = ConceptId::parse("a").unwrap();
    let b = ConceptId::parse("b").unwrap();
    assert!(bundle.backlinks(&a).is_empty());
    match remove_concept(&bundle, &a, &RemoveOptions::default()) {
        Err(RefactorError::HasInboundLinks {
            inbound_concepts, ..
        }) => assert_eq!(inbound_concepts, vec![ConceptId::parse("old").unwrap()]),
        other => panic!("expected the removal to be refused, got {other:?}"),
    }

    // With a redirect, the field follows.
    remove_concept(
        &bundle,
        &a,
        &RemoveOptions {
            redirect_to: Some(b),
            ..RemoveOptions::default()
        },
    )
    .unwrap();
    assert!(!tmp.path().join("a.md").exists());
    assert!(tmp.read("old.md").contains("superseded_by: b.md\n"));
}

#[test]
fn test_merge_and_split_keep_unknown_keys_and_untouched_entries() {
    let tmp = TempDir::new();
    tmp.write("index.md", "---\nokf_version: \"0.3\"\n---\n\n# Bundle\n");
    tmp.write("policies/p.md", "---\ntype: Policy\n---\n\n# P\n");
    tmp.write(
        "billing/discounts.md",
        "---\ntype: Concept\ntitle: Discounts\n\
         verified: { by: \"human:bob\", at: \"2026-02-01T00:00:00Z\", x-acme-ticket: FIN-7 }\n\
         sources:\n  - { id: policy, resource: ../policies/p.md, x-acme-weight: 3 }\n\
         ---\n\n# Discounts\n\nPer policy.[^policy]\n\n[^policy]: Policy\n",
    );
    let target = "---\n\
        type: Concept\n\
        title: Pricing\n\
        tags: [billing, pricing]   # kept\n\
        generated: { by: reference_agent/v1, at: \"2026-01-01T00:00:00Z\" }\n\
        x-acme-owner: finance\n\
        ---\n\n# Pricing\n\nPrices.\n\n## Enterprise\n\nEnterprise terms.\n";
    tmp.write("pricing.md", target);

    let bundle = Bundle::load(tmp.path()).unwrap();
    let pricing = ConceptId::parse("pricing").unwrap();
    merge_concepts(
        &bundle,
        &ConceptId::parse("billing/discounts").unwrap(),
        &pricing,
        &MergeOptions::default(),
    )
    .unwrap();

    let merged = tmp.read("pricing.md");
    // Entries the merge did not touch are byte for byte what they were.
    assert!(
        merged.contains("tags: [billing, pricing]   # kept\n"),
        "{merged}"
    );
    assert!(merged.contains("x-acme-owner: finance\n"), "{merged}");
    // Merged-in entries keep the keys this crate does not know, and a path
    // relative to the old location is rebased to the new one.
    assert!(merged.contains("x-acme-ticket: FIN-7"), "{merged}");
    assert!(merged.contains("x-acme-weight: 3"), "{merged}");
    assert!(merged.contains("resource: policies/p.md"), "{merged}");

    // The target says something new, so its `generated.at` moves to now and
    // the review that came with the merged concept is history: it was of
    // another text. The producer is still who it was.
    let bundle = Bundle::load(tmp.path()).unwrap();
    let fm = &bundle.get(&pricing).unwrap().document.frontmatter;
    let generated = fm.generated().unwrap();
    assert_eq!(generated.by.unwrap().as_str(), "reference_agent/v1");
    assert_ne!(generated.at.unwrap().raw, "2026-01-01T00:00:00Z");
    assert_eq!(fm.verified().len(), 1);
    assert_eq!(fm.trust_tier(), okf_core::TrustTier::Unverified);
    assert!(fm.verified_before_change());

    // Splitting leaves the frontmatter of the original alone when no source
    // moves out with the section.
    let frontmatter_before = merged.split("\n---\n").next().unwrap().to_string();
    split_concept(
        &bundle,
        &pricing,
        &ConceptId::parse("enterprise").unwrap(),
        &SplitOptions {
            section: "Enterprise".to_string(),
            ..SplitOptions::default()
        },
    )
    .unwrap();
    let after_split = tmp.read("pricing.md");
    assert_eq!(
        after_split.split("\n---\n").next().unwrap(),
        frontmatter_before
    );
    assert!(tmp.read("enterprise.md").contains("Enterprise terms."));
}

#[test]
fn test_links_to_assets_and_to_the_concept_itself_survive_a_move() {
    let tmp = TempDir::new();
    tmp.write("index.md", "---\nokf_version: \"0.3\"\n---\n\n# Bundle\n");
    tmp.write("tables/img/a.png", "png");
    tmp.write("tables/q.sql", "SELECT 1\n");
    tmp.write("tables/my notes.md", "---\ntype: Note\n---\n\n# Notes\n");
    tmp.write(
        "tables.md",
        "---\ntype: Note\n---\n\n# A concept named like the directory\n",
    );
    tmp.write(
        "tables/orders.md",
        "---\ntype: Table\n---\n\n# Orders\n\n\
         ![diagram](img/a.png) and the [query](q.sql). See [below](orders.md#keys),\n\
         the [same](/tables/orders.md), a [draft](soon.md), and the [notes](my%20notes.md).\n\n## Keys\n",
    );
    tmp.write(
        "overview.md",
        "---\ntype: Note\n---\n\nThe [tables directory](tables), the [orders](tables/orders),\n\
         and the [notes](tables/my%20notes.md).\n",
    );

    let bundle = Bundle::load(tmp.path()).unwrap();
    let overview = ConceptId::parse("overview").unwrap();
    // A link to a directory is not a link to the concept that shares its
    // name; a target that names nothing may still be a concept id.
    let kinds: Vec<(String, okf_core::LinkTarget)> = bundle
        .links_from(&overview)
        .iter()
        .map(|link| (link.raw.clone(), link.resolution))
        .collect();
    assert_eq!(
        kinds,
        vec![
            ("tables".to_string(), okf_core::LinkTarget::File),
            ("tables/orders".to_string(), okf_core::LinkTarget::Concept),
            (
                "tables/my%20notes.md".to_string(),
                okf_core::LinkTarget::Concept
            ),
        ]
    );

    move_concept(
        &bundle,
        &ConceptId::parse("tables/orders").unwrap(),
        &ConceptId::parse("sales/orders2").unwrap(),
        &MoveOptions::default(),
    )
    .unwrap();
    let moved = tmp.read("sales/orders2.md");
    // Assets keep their extension, and are found from the new place.
    assert!(moved.contains("![diagram](../tables/img/a.png)"), "{moved}");
    assert!(moved.contains("[query](../tables/q.sql)"), "{moved}");
    // Links to itself follow the concept to its new name.
    assert!(moved.contains("[below](orders2.md#keys)"), "{moved}");
    assert!(moved.contains("[same](/sales/orders2.md)"), "{moved}");
    // A file that is not written yet is rebased as a path.
    assert!(moved.contains("[draft](../tables/soon.md)"), "{moved}");
    assert!(
        moved.contains("[notes](../tables/my%20notes.md)"),
        "{moved}"
    );
    let overview_text = tmp.read("overview.md");
    assert!(
        overview_text.contains("[tables directory](tables)"),
        "{overview_text}"
    );
    assert!(
        overview_text.contains("[orders](sales/orders2.md)"),
        "{overview_text}"
    );

    // A percent-encoded incoming link follows a move too.
    let bundle = Bundle::load(tmp.path()).unwrap();
    move_concept(
        &bundle,
        &ConceptId::parse("tables/my notes").unwrap(),
        &ConceptId::parse("tables/notes").unwrap(),
        &MoveOptions::default(),
    )
    .unwrap();
    assert!(tmp.read("overview.md").contains("[notes](tables/notes.md)"));
    assert!(
        tmp.read("sales/orders2.md")
            .contains("[notes](../tables/notes.md)")
    );
}

#[test]
fn test_refactors_do_not_overwrite_files_that_are_not_concepts() {
    let tmp = TempDir::new();
    tmp.write("index.md", "---\nokf_version: \"0.3\"\n---\n\n# Bundle\n");
    tmp.write("NOTES.md", "# Hand-written notes\n");
    tmp.write("a.md", "---\ntype: Note\n---\n\n# A\n\n## Part\n\nText.\n");
    tmp.write("b.md", "---\ntype: Note\n---\n\nSee [a](a.md).\n");
    let bundle = Bundle::load(tmp.path()).unwrap();
    let a = ConceptId::parse("a").unwrap();
    let notes = ConceptId::parse("NOTES").unwrap();

    // `NOTES.md` has no `type`, so it is not a concept, but it is a file.
    assert!(bundle.get(&notes).is_none());
    assert_eq!(
        move_concept(&bundle, &a, &notes, &MoveOptions::default()),
        Err(RefactorError::ConceptAlreadyExists(notes.clone()))
    );
    let split = SplitOptions {
        section: "Part".to_string(),
        ..SplitOptions::default()
    };
    assert_eq!(
        split_concept(&bundle, &a, &notes, &split).unwrap_err(),
        RefactorError::ConceptAlreadyExists(notes)
    );
    assert_eq!(tmp.read("NOTES.md"), "# Hand-written notes\n");

    // A reserved name can never hold a concept, forced or not.
    for reserved in ["index", "sub/log"] {
        let target = ConceptId::parse(reserved).unwrap();
        let forced = MoveOptions {
            force: true,
            ..MoveOptions::default()
        };
        assert!(matches!(
            move_concept(&bundle, &a, &target, &forced),
            Err(RefactorError::InvalidConceptId(_))
        ));
    }
    assert!(tmp.read("index.md").starts_with("---\nokf_version"));

    // Redirecting links to the concept being removed is refused.
    let redirect_to_self = RemoveOptions {
        redirect_to: Some(a.clone()),
        ..RemoveOptions::default()
    };
    assert_eq!(
        remove_concept(&bundle, &a, &redirect_to_self).unwrap_err(),
        RefactorError::SameSourceAndTarget(a)
    );
    assert!(tmp.path().join("a.md").exists());
}
