//! Document parsing, serialization, and validation tests.
//!
//! These mirror the reference implementation's `tests/test_document.py` to
//! guarantee behavioural parity, plus extra edge cases. Where the reference
//! calls a free function on a raw frontmatter dict (`normalize_verified`,
//! `trust_tier`, `is_stale`), the equivalent here is a [`Frontmatter`] accessor,
//! so the assertions are transcribed rather than the call shapes.

use okf::yaml::Value;
use okf::{Date, Document, DocumentError, TrustTier};

/// Builds a document from a frontmatter block with an empty body.
fn with_frontmatter(frontmatter: &str) -> Document {
    Document::parse(&format!("---\n{frontmatter}\n---\n")).unwrap()
}

#[test]
fn roundtrip_preserves_frontmatter_and_body() {
    let src = "---\n\
        type: BigQuery Table\n\
        title: Sample\n\
        description: A sample table.\n\
        tags: [a, b]\n\
        timestamp: 2026-05-27T00:00:00+00:00\n\
        ---\n\
        \n\
        # Sample\n\
        \n\
        Body text.\n";
    let doc = Document::parse(src).unwrap();
    assert_eq!(doc.frontmatter.type_().as_deref(), Some("BigQuery Table"));
    assert_eq!(doc.frontmatter.tags(), vec!["a", "b"]);
    assert!(doc.body.starts_with("# Sample"));

    let serialized = doc.serialize();
    let reparsed = Document::parse(&serialized).unwrap();
    assert_eq!(reparsed.frontmatter, doc.frontmatter);
    assert_eq!(reparsed.body.trim(), doc.body.trim());
}

#[test]
fn parse_no_frontmatter_treats_all_as_body() {
    let src = "# Hello\n\nNo frontmatter here.\n";
    let doc = Document::parse(src).unwrap();
    assert!(doc.frontmatter.is_empty());
    assert!(doc.body.contains("Hello"));
}

#[test]
fn unterminated_frontmatter_raises() {
    let src = "---\ntype: X\nstill in frontmatter\n";
    let err = Document::parse(src).unwrap_err();
    assert_eq!(err, DocumentError::UnterminatedFrontmatter);
}

#[test]
fn validate_rejects_missing_type() {
    let doc = with_frontmatter("title: Y");
    let err = doc.validate().unwrap_err();
    assert!(err.to_string().contains("type"), "{err}");
}

#[test]
fn validate_accepts_type_only() {
    // §11: `type` is the only always-required key.
    assert!(with_frontmatter("type: X").validate().is_ok());
}

#[test]
fn an_empty_type_does_not_count_as_present() {
    assert!(with_frontmatter("type: \"\"").validate().is_err());
}

#[test]
fn missing_recommended_is_the_producer_checklist_not_a_rejection() {
    let sparse = with_frontmatter("type: X\ntitle: Y");
    assert_eq!(sparse.missing_recommended(), ["description", "generated"]);
    // Every one of them is optional, so the document still conforms (§11).
    assert!(sparse.validate().is_ok());

    let full = with_frontmatter(
        "type: X\ntitle: Y\ndescription: Z\n\
         generated: { by: reference_agent/gemini-2.5-pro, at: 2026-06-20T22:53:05Z }",
    );
    assert!(full.missing_recommended().is_empty());
}

#[test]
fn a_legacy_timestamp_stands_in_for_generated() {
    // §13.1: consumers MAY fall back to a v0.1 `timestamp` when `generated` is
    // absent, so a v0.1 document has nothing left on the checklist.
    let doc =
        with_frontmatter("type: X\ntitle: Y\ndescription: Z\ntimestamp: 2026-05-27T00:00:00+00:00");
    assert!(doc.missing_recommended().is_empty());
    assert_eq!(
        doc.frontmatter.content_changed_at().unwrap().raw,
        "2026-05-27T00:00:00+00:00"
    );
}

#[test]
fn an_attested_computation_should_carry_a_runtime() {
    let without = with_frontmatter(
        "type: Attested Computation\ntitle: Revenue\ndescription: Recognized revenue.\n\
         generated: { by: human:ahormati, at: 2026-06-20T22:53:05Z }",
    );
    assert_eq!(without.missing_recommended(), ["runtime"]);
    // §10.2's `runtime` is a SHOULD like the rest of the families (§11).
    assert!(without.validate().is_ok());

    let with = with_frontmatter(
        "type: Attested Computation\ntitle: Revenue\ndescription: Recognized revenue.\n\
         runtime: bigquery\n\
         generated: { by: human:ahormati, at: 2026-06-20T22:53:05Z }",
    );
    assert!(with.missing_recommended().is_empty());
}

#[test]
fn a_bare_verified_mapping_is_read_as_a_one_element_list() {
    let doc =
        with_frontmatter("type: X\nverified: { by: human:ahormati, at: 2026-06-25T09:00:00Z }");
    let events = doc.frontmatter.verified();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].by.as_ref().unwrap().as_str(), "human:ahormati");
    assert_eq!(events[0].at.as_ref().unwrap().raw, "2026-06-25T09:00:00Z");

    assert!(with_frontmatter("type: X")
        .frontmatter
        .verified()
        .is_empty());
}

#[test]
fn trust_tiers_derive_from_the_verifying_actors() {
    // The tier strings are part of the interchange vocabulary (§5.3), so assert
    // on them verbatim, not just on the enum.
    let tier = |frontmatter: &str| with_frontmatter(frontmatter).frontmatter.trust_tier();

    assert_eq!(tier("type: X"), TrustTier::Unverified);
    assert_eq!(tier("type: X").to_string(), "unverified");

    let machine = tier("type: X\nverified: [{ by: process:finance-nightly, at: x }]");
    assert_eq!(machine, TrustTier::MachineConfirmed);
    assert_eq!(machine.to_string(), "machine-confirmed");

    let both = tier(
        "type: X\nverified:\n  - { by: process:finance-nightly, at: x }\n  \
         - { by: human:ahormati, at: y }",
    );
    assert_eq!(both, TrustTier::HumanReviewed);
    assert_eq!(both.to_string(), "human-reviewed");

    // A bare mapping is treated as a one-element list.
    assert_eq!(
        tier("type: X\nverified: { by: human:ahormati, at: z }"),
        TrustTier::HumanReviewed
    );
}

#[test]
fn staleness_compares_stale_after_against_a_given_day() {
    let today = Date::new(2026, 9, 23).unwrap();
    let stale = |frontmatter: &str| with_frontmatter(frontmatter).frontmatter.is_stale_on(today);

    assert!(
        stale("type: X\nstale_after: 2026-09-23"),
        "stale on the day itself"
    );
    assert!(!stale("type: X\nstale_after: 2026-09-24"));
    assert!(!stale("type: X"));
    assert!(!stale("type: X\nstale_after: not-a-date"));
}

#[test]
fn unknown_keys_are_preserved_on_roundtrip() {
    let src = "---\ntype: X\ncustom_key: custom value\nnested:\n  a: 1\n  b: 2\n---\nbody\n";
    let doc = Document::parse(src).unwrap();
    assert!(doc.frontmatter.get("custom_key").is_some());
    let extensions = doc.frontmatter.extension_keys();
    assert!(extensions.contains(&"custom_key"));
    assert!(extensions.contains(&"nested"));

    let reparsed = Document::parse(&doc.serialize()).unwrap();
    assert_eq!(reparsed.frontmatter, doc.frontmatter);
    assert_eq!(
        reparsed.frontmatter.get("nested"),
        Some(&Value::parse("{a: 1, b: 2}").unwrap())
    );
}

#[test]
fn empty_frontmatter_block_is_empty_mapping() {
    let doc = Document::parse("---\n---\nbody\n").unwrap();
    assert!(doc.frontmatter.is_empty());
    // The trailing newline is dropped on parse (matching the reference's
    // splitlines/join); serialize restores it.
    assert_eq!(doc.body, "body");
    assert!(doc.serialize().ends_with("body\n"));
}

#[test]
fn a_datetime_valued_stale_after_is_compared_on_its_date() {
    // §5.5 asks for "an absolute date (`YYYY-MM-DD`)", so a datetime there is a
    // deviation the validator reports. It still has to be compared, though, and
    // the reference truncates to the first ten characters rather than treating
    // the concept as fresh forever.
    let doc = with_frontmatter("type: X\nstale_after: '2026-09-23T00:00:00Z'");
    let field = doc.frontmatter.stale_after().unwrap();
    assert!(!field.is_valid(), "still reported as not a plain date");
    assert_eq!(field.effective_date(), Date::new(2026, 9, 23));

    assert!(doc.frontmatter.is_stale_on(Date::new(2026, 9, 23).unwrap()));
    assert!(!doc.frontmatter.is_stale_on(Date::new(2026, 9, 22).unwrap()));
}

#[test]
fn reorder_preferred_matches_the_reference_key_order() {
    let mut doc = with_frontmatter(
        "custom_key: keep me\nsources: []\ntitle: Orders\ntype: BigQuery Table\nstatus: stable",
    );
    doc.frontmatter.reorder_preferred();

    let keys: Vec<&str> = doc.frontmatter.as_mapping().keys().collect();
    assert_eq!(keys, ["type", "title", "status", "sources", "custom_key"]);
    // Reordering neither drops nor rewrites anything.
    assert_eq!(
        doc.frontmatter.get("custom_key").unwrap().as_str(),
        Some("keep me")
    );
    assert_eq!(doc.frontmatter.as_mapping().len(), 5);
}

#[test]
fn section_reads_the_lines_under_a_conventional_heading() {
    // §4.2's conventional headings carry no required behaviour, so this is the
    // primitive for reading them. Ported from the reference's
    // `_section_content_lines`, whose details this locks in.
    let doc = Document::parse(
        "---\ntype: BigQuery Table\n---\n\n\
         Prose.\n\n\
         # Schema\n\n\
         - `id` STRING: the order id\n\
         \n\
         ## Nested\n\
         - `total` NUMERIC: order total\n\n\
         # Examples\n\
         \x20   SELECT 1\n",
    )
    .unwrap();

    // Blank lines are dropped, a `##` subheading stays inside the section, and
    // each line keeps its own indentation.
    assert_eq!(
        doc.section("# Schema"),
        [
            "- `id` STRING: the order id",
            "## Nested",
            "- `total` NUMERIC: order total",
        ]
    );
    assert_eq!(doc.section("# Examples"), ["    SELECT 1"]);
    assert!(doc.section("# Computation").is_empty());
    // The heading is matched in full, so the leading `# ` is required.
    assert!(doc.section("Schema").is_empty());
}

#[test]
fn a_non_string_type_is_coerced_not_dropped() {
    // §4.1 calls `type` "a short string", so `type: 42` is a producer
    // deviation, but the typed accessors coerce non-string scalars to their
    // display form the way the reference's `str(fm.get("type"))` does, rather
    // than returning `None` and leaving a `validate()`-passing concept
    // typeless. Locking this in so a "borrow-only" refactor does not silently
    // reintroduce the regression.
    let doc = with_frontmatter("type: 42\ntitle: hello");
    assert_eq!(doc.frontmatter.type_().as_deref(), Some("42"));
    // `validate()` only checks `type` is non-empty, so the deviation still
    // conforms (§11). A future validator rule could reject it, but the
    // accessor's contract is to surface what was written, not to legislate.
    assert!(doc.validate().is_ok());

    // Booleans and floats coerce the same way.
    assert_eq!(
        with_frontmatter("type: true")
            .frontmatter
            .type_()
            .as_deref(),
        Some("true")
    );
    assert_eq!(
        with_frontmatter("type: 1.5").frontmatter.type_().as_deref(),
        Some("1.5")
    );
    // A non-scalar (sequence) is not a plausible `type` and yields `None`.
    assert!(with_frontmatter("type: [a, b]")
        .frontmatter
        .type_()
        .is_none());
}

#[test]
fn line_endings_match_the_reference_parser() {
    // The reference implementation's two parse paths handle line endings
    // differently, and this crate mirrors that by deliberate parity:
    //
    //  * no frontmatter -> `return cls(body=text)`: the body is kept verbatim,
    //    so CRLF round-trips byte-identically;
    //  * frontmatter    -> `splitlines()` + `"\n".join(...)`: the body (and
    //    the frontmatter text) is normalized to `\n`.
    //
    // Locking the behaviour in so a "fix" that normalizes both paths does not
    // silently break parity with the reference bundles.

    // No frontmatter: CRLF is preserved verbatim.
    let src = "# Hello\r\n\r\nNo frontmatter here.\r\n";
    let doc = Document::parse(src).unwrap();
    assert!(doc.frontmatter.is_empty());
    assert_eq!(doc.body, src);

    // With frontmatter: CRLF in the body is normalized to LF, and the `\r` on
    // the closing `---` is stripped so the delimiter is still recognized.
    let src = "---\r\ntype: X\r\n---\r\n\r\n# Hello\r\n\r\nBody.\r\n";
    let doc = Document::parse(src).unwrap();
    assert_eq!(doc.frontmatter.type_().as_deref(), Some("X"));
    assert_eq!(doc.body, "# Hello\n\nBody.");

    // Re-serializing always emits `\n`; a CRLF file that went through the
    // frontmatter path therefore does not round-trip byte-identically,
    // matching the reference's `serialize()` which joins on `"\n"`. Asserting
    // both the absence of CR and that the bytes differ, so a future
    // `serialize()` change that no-op'd the input still fails this test.
    let serialized = doc.serialize();
    assert!(
        !serialized.contains('\r'),
        "serialize must not emit CR: {serialized:?}"
    );
    assert_ne!(serialized, src, "serialize must not round-trip CRLF");
}
