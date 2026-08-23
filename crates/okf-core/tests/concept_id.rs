//! Concept-id segment rules (§2).
//!
//! The spec states no character constraint on filenames, so `validate_segment`
//! rejects only what cannot be a concept id. The reference implementation's
//! ASCII convention survives as `is_portable_segment`, which drives a warning
//! rather than a parse error.

use okf_core::ConceptId;
use okf_core::concept_id::is_portable_segment;

#[test]
fn accepts_names_the_reference_ascii_rule_rejected() {
    for id in [
        "tables/my notes",
        "tables/rocket\u{1f680}",
        "notities/caf\u{e9}",
        "\u{8868}/\u{7528}\u{6237}",
        "tables/orders (v2)",
        "a b/c d/e f",
        "-leading-hyphen",
        ".hidden",
    ] {
        let parsed = ConceptId::parse(id);
        assert!(parsed.is_ok(), "rejected {id:?}: {parsed:?}");
        assert_eq!(parsed.unwrap().to_string(), id);
    }
}

#[test]
fn still_rejects_what_cannot_be_a_segment() {
    for id in ["", "/", "//", ".", "..", "a/./b", "a/../b"] {
        assert!(
            ConceptId::parse(id).is_err(),
            "should have been rejected: {id:?}"
        );
    }
    // Separators and control characters, which `parse` cannot see because it
    // splits on `/`, are caught when segments are supplied directly.
    for seg in ["a\\b", "a\nb", "a\0b", "", "..", "."] {
        assert!(
            ConceptId::new(vec![seg.to_string()]).is_err(),
            "should have been rejected: {seg:?}"
        );
    }
}

#[test]
fn an_id_survives_a_round_trip_through_its_string_form() {
    for id in ["tables/users", "tables/my notes", "a/b/rocket\u{1f680}"] {
        let parsed = ConceptId::parse(id).unwrap();
        assert_eq!(ConceptId::parse(&parsed.to_string()), Ok(parsed));
    }
}

#[test]
fn to_path_keeps_one_segment_to_one_component() {
    let root = std::path::Path::new("/bundle");
    let path = ConceptId::parse("tables/my notes").unwrap().to_path(root);
    assert_eq!(path, std::path::Path::new("/bundle/tables/my notes.md"));
    assert_eq!(path.components().count(), 4);
}

#[test]
fn from_path_round_trips_valid_paths_and_rejects_non_markdown_paths() {
    let root = std::path::Path::new("/bundle");
    let path = root.join("tables/my notes.md");
    let id = ConceptId::from_path(root, &path).unwrap();
    assert_eq!(id, ConceptId::parse("tables/my notes").unwrap());
    assert_eq!(id.to_path(root), path);

    for path in [
        root.join("../outside.md"),
        root.join("tables/../../outside.md"),
        root.join("tables/my notes.txt"),
    ] {
        assert!(
            ConceptId::from_path(root, &path).is_err(),
            "accepted invalid path: {}",
            path.display()
        );
    }
}

#[cfg(unix)]
#[test]
fn from_path_rejects_non_utf8_segments_instead_of_replacing_them() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    let root = std::path::Path::new("/bundle");
    let path = root.join(OsString::from_vec(vec![0xff, b'.', b'm', b'd']));
    assert!(ConceptId::from_path(root, &path).is_err());
}

#[test]
fn portability_matches_the_reference_convention() {
    for seg in ["users", "my_table", "v0.2", "a-b", "_x", "A1"] {
        assert!(is_portable_segment(seg), "{seg:?} should be portable");
    }
    for seg in [
        "my notes",
        "caf\u{e9}",
        "rocket\u{1f680}",
        "-leading",
        ".hidden",
        "",
    ] {
        assert!(!is_portable_segment(seg), "{seg:?} should not be portable");
    }
}
