//! Explicit tests for section anchor links across refactoring operations.

mod common;

use common::TempDir;
use okf_core::Bundle;
use okf_core::concept_id::ConceptId;
use okf_core::refactor::{MoveOptions, move_concept};

#[test]
fn test_move_concept_preserves_all_section_hashtag_formats() {
    let tmp = TempDir::new();
    tmp.write(
        "index.md",
        "---\nokf_version: \"0.2\"\n---\n\n# Test Bundle\n",
    );
    tmp.write(
        "auth/token.md",
        "---\ntype: Concept\ntitle: Auth Token\n---\n\n# Auth Token\n\n## Expiration\nToken expires after 1h.\n\n## Refresh\nRefresh token details.\n",
    );
    tmp.write(
        "users/profile.md",
        "---\ntype: Concept\ntitle: User Profile\n---\n\n# User Profile\n\n- Relative dot-slash: [Token Exp](./../auth/token#expiration)\n- Relative with md: [Token Refresh](../auth/token.md#refresh)\n- Direct relative: [Direct](./token#expiration)\n- Absolute root: [Absolute](/auth/token.md#expiration)\n- With title: [Titled](../auth/token.md#expiration \"Auth Exp Title\")\n",
    );

    let bundle = Bundle::load(tmp.path()).unwrap();
    let from = ConceptId::parse("auth/token").unwrap();
    let to = ConceptId::parse("security/jwt/token").unwrap();

    let opts = MoveOptions::default();
    let report = move_concept(&bundle, &from, &to, &opts).unwrap();

    assert_eq!(report.rewritten_incoming_links, 4);

    let users_content = tmp.read("users/profile.md");
    // Verify each anchor format was updated to point to the new location while keeping the exact hashtag
    assert!(
        users_content.contains("[Token Exp](../security/jwt/token.md#expiration)"),
        "Content was:\n{users_content}"
    );
    assert!(
        users_content.contains("[Token Refresh](../security/jwt/token.md#refresh)"),
        "Content was:\n{users_content}"
    );
    assert!(
        users_content.contains("[Absolute](/security/jwt/token.md#expiration)"),
        "Content was:\n{users_content}"
    );
    assert!(
        users_content.contains("[Titled](../security/jwt/token.md#expiration \"Auth Exp Title\")"),
        "Content was:\n{users_content}"
    );
}

#[test]
fn test_in_document_anchors_inside_moved_concept_are_unmodified() {
    let tmp = TempDir::new();
    tmp.write("index.md", "---\nokf_version: \"0.2\"\n---\n\n# Bundle\n");
    tmp.write(
        "auth/token.md",
        "---\ntype: Concept\ntitle: Auth Token\n---\n\n# Auth Token\n\nSee [Internal Expiration](#expiration) within this document.\n\n## Expiration\nDetails.\n",
    );

    let bundle = Bundle::load(tmp.path()).unwrap();
    let from = ConceptId::parse("auth/token").unwrap();
    let to = ConceptId::parse("security/jwt").unwrap();

    move_concept(&bundle, &from, &to, &MoveOptions::default()).unwrap();

    let moved = tmp.read("security/jwt.md");
    assert!(moved.contains("[Internal Expiration](#expiration)"));
}
