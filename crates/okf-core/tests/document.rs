//! Document parsing, serialization, and validation tests.
//!
//! These mirror the reference implementation's `tests/test_document.py` to
//! guarantee behavioural parity, plus extra edge cases. Where the reference
//! calls a free function on a raw frontmatter dict (`normalize_verified`,
//! `trust_tier`, `is_stale`), the equivalent here is a [`Frontmatter`] accessor,
//! so the assertions are transcribed rather than the call shapes.

use okf_core::yaml::Value;
use okf_core::{Date, Document, DocumentError, Freshness, TrustTier};

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
    // `type` is the only always-required key.
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
    // Every one of them is optional, so the document still conforms.
    assert!(sparse.validate().is_ok());

    let full = with_frontmatter(
        "type: X\ntitle: Y\ndescription: Z\n\
         generated: { by: reference_agent/gemini-2.5-pro, at: 2026-06-20T22:53:05Z }",
    );
    assert!(full.missing_recommended().is_empty());
}

#[test]
fn a_legacy_timestamp_stands_in_for_generated() {
    // Consumers MAY fall back to a v0.1 `timestamp` when `generated` is
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
         generated: { by: human:walter, at: 2026-06-20T22:53:05Z }",
    );
    assert_eq!(without.missing_recommended(), ["runtime"]);
    // `runtime` is a SHOULD like the rest of the families.
    assert!(without.validate().is_ok());

    let with = with_frontmatter(
        "type: Attested Computation\ntitle: Revenue\ndescription: Recognized revenue.\n\
         runtime: bigquery\n\
         generated: { by: human:walter, at: 2026-06-20T22:53:05Z }",
    );
    assert!(with.missing_recommended().is_empty());
}

#[test]
fn a_bare_verified_mapping_is_read_as_a_one_element_list() {
    let doc = with_frontmatter("type: X\nverified: { by: human:walter, at: 2026-06-25T09:00:00Z }");
    let events = doc.frontmatter.verified();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].by.as_ref().unwrap().as_str(), "human:walter");
    assert_eq!(events[0].at.as_ref().unwrap().raw, "2026-06-25T09:00:00Z");

    assert!(
        with_frontmatter("type: X")
            .frontmatter
            .verified()
            .is_empty()
    );
}

#[test]
fn trust_tiers_derive_from_the_verifying_actors() {
    // The tier strings are part of the interchange vocabulary, so assert
    // on them verbatim, not just on the enum.
    let tier = |frontmatter: &str| with_frontmatter(frontmatter).frontmatter.trust_tier();

    assert_eq!(tier("type: X"), TrustTier::Unverified);
    assert_eq!(tier("type: X").to_string(), "unverified");

    let machine =
        tier("type: X\nverified: [{ by: process:finance-nightly, at: 2026-06-26T02:00:00Z }]");
    assert_eq!(machine, TrustTier::MachineConfirmed);
    assert_eq!(machine.to_string(), "machine-confirmed");

    let both = tier(
        "type: X\nverified:\n  - { by: process:finance-nightly, at: 2026-06-26T02:00:00Z }\n  \
         - { by: human:walter, at: 2026-06-25T09:00:00Z }",
    );
    assert_eq!(both, TrustTier::HumanReviewed);
    assert_eq!(both.to_string(), "human-reviewed");

    // A bare mapping is treated as a one-element list.
    assert_eq!(
        tier("type: X\nverified: { by: human:walter, at: 2026-06-25T09:00:00Z }"),
        TrustTier::HumanReviewed
    );
}

#[test]
fn staleness_compares_stale_after_against_a_given_day() {
    let today = Date::new(2026, 9, 23).unwrap();
    let stale = |frontmatter: &str| with_frontmatter(frontmatter).frontmatter.is_stale_on(today);
    let freshness = |frontmatter: &str| {
        with_frontmatter(frontmatter)
            .frontmatter
            .freshness_at(today.to_utc_datetime())
    };

    assert!(
        stale("type: X\nstale_after: '2026-09-23T00:00:00Z'"),
        "stale on the instant itself"
    );
    assert!(!stale("type: X\nstale_after: '2026-09-24T00:00:00Z'"));
    assert_eq!(freshness("type: X"), Freshness::Undated);
    assert!(!stale("type: X"));
    // A null or empty value is the same as an absent key.
    assert_eq!(freshness("type: X\nstale_after:"), Freshness::Undated);
    assert_eq!(freshness("type: X\nstale_after: ''"), Freshness::Undated);

    // A date only is the start of that day in UTC, quoted or not.
    assert!(stale("type: X\nstale_after: 2026-09-23"));
    assert!(stale("type: X\nstale_after: \"2026-09-23\""));
    assert_eq!(
        freshness("type: X\nstale_after: 2026-09-24"),
        Freshness::Fresh
    );

    // A value that does not parse is never treated as fresh.
    for malformed in [
        "not-a-date",
        "'2026-09-23T00:00:00'",
        "'2099-01-01 00:00:00Z'",
        "20990101",
    ] {
        let frontmatter = format!("type: X\nstale_after: {malformed}");
        assert_eq!(freshness(&frontmatter), Freshness::Malformed, "{malformed}");
        assert!(stale(&frontmatter), "{malformed}");
    }
}

#[test]
fn trust_tier_counts_only_current_verifications() {
    let tier = |frontmatter: &str| with_frontmatter(frontmatter).frontmatter.trust_tier();
    let generated = "generated: { by: reference_agent/x, at: \"2026-06-20T14:00:00Z\" }";

    // Reviewed, then rewritten: the human did not read the new text.
    let rewritten = with_frontmatter(&format!(
        "type: X\n{generated}\nverified: {{ by: \"human:ahormati\", at: \"2026-06-01T09:00:00Z\" }}"
    ));
    assert_eq!(rewritten.frontmatter.trust_tier(), TrustTier::Unverified);
    assert!(rewritten.frontmatter.verified_before_change());
    // The event is still there as history.
    assert_eq!(rewritten.frontmatter.verified().len(), 1);
    assert!(rewritten.frontmatter.current_verifications().is_empty());
    assert!(rewritten.frontmatter.latest_verification().is_some());

    // A later machine check is current; the earlier human review is not.
    assert_eq!(
        tier(&format!(
            "type: X\n{generated}\nverified:\n  - {{ by: \"human:ahormati\", at: \"2026-06-01T09:00:00Z\" }}\n  - {{ by: \"process:nightly\", at: \"2026-06-21T02:00:00+00:00\" }}"
        )),
        TrustTier::MachineConfirmed
    );
    // Equal instants are current.
    assert_eq!(
        tier(&format!(
            "type: X\n{generated}\nverified: {{ by: \"human:ahormati\", at: \"2026-06-20T16:00:00+02:00\" }}"
        )),
        TrustTier::HumanReviewed
    );
    // With no `generated`, every valid event is current.
    let ungenerated =
        with_frontmatter("type: X\nverified: { by: \"human:ahormati\", at: 2026-06-01 }");
    assert_eq!(
        ungenerated.frontmatter.trust_tier(),
        TrustTier::HumanReviewed
    );
    assert!(!ungenerated.frontmatter.verified_before_change());
    // A legacy v0.1 `timestamp` stands in for `generated.at`.
    assert_eq!(
        tier(
            "type: X\ntimestamp: \"2026-06-20T14:00:00Z\"\nverified: { by: \"human:a\", at: \"2026-06-01T09:00:00Z\" }"
        ),
        TrustTier::Unverified
    );
    // An event without both keys is ignored.
    let incomplete =
        with_frontmatter("type: X\nverified:\n  - { by: \"human:a\" }\n  - { at: 2026-06-01 }");
    assert!(incomplete.frontmatter.verified().is_empty());
    assert_eq!(incomplete.frontmatter.trust_tier(), TrustTier::Unverified);
}

#[test]
fn absent_empty_and_single_values_read_by_the_consumer_rules() {
    // A scalar `tags` value is one tag, and is not split at commas.
    let scalar = with_frontmatter("type: X\ntags: sales, orders");
    assert_eq!(scalar.frontmatter.tags(), vec!["sales, orders"]);
    assert_eq!(
        with_frontmatter("type: X\ntags: [sales, \"\", orders]")
            .frontmatter
            .tags(),
        vec!["sales", "orders"]
    );
    // Null, an empty string, and an empty list are the same as an absent key.
    for blank in ["tags:", "tags: \"\"", "tags: []"] {
        assert!(
            with_frontmatter(&format!("type: X\n{blank}"))
                .frontmatter
                .tags()
                .is_empty(),
            "{blank}"
        );
    }
    let blank = with_frontmatter("type: X\ntitle: \"\"\ndescription:\nresource: ''");
    assert_eq!(blank.frontmatter.title(), None);
    assert_eq!(blank.frontmatter.description(), None);
    assert_eq!(blank.frontmatter.resource(), None);

    // An entry that lacks its required key is ignored.
    let doc = with_frontmatter(
        "type: X\nsources:\n  - { id: kept, resource: https://example.com/a }\n  - { id: dropped, title: No resource }\n",
    );
    let sources = doc.frontmatter.sources();
    assert_eq!(sources.len(), 1);
    assert_eq!(sources[0].id.as_deref(), Some("kept"));
    // One source is a list with one item.
    assert_eq!(
        with_frontmatter("type: X\nsources: { resource: https://example.com/a }")
            .frontmatter
            .sources()
            .len(),
        1
    );
}

#[test]
fn lifecycle_reads_superseded_by_and_unknown_statuses() {
    let retired =
        with_frontmatter("type: X\nstatus: deprecated\nsuperseded_by: /metrics/gross-margin.md");
    assert!(!retired.frontmatter.is_current());
    assert_eq!(
        retired.frontmatter.superseded_by().as_deref(),
        Some("/metrics/gross-margin.md")
    );
    assert!(retired.frontmatter.path_fields().contains(&(
        "superseded_by".to_string(),
        "/metrics/gross-margin.md".to_string()
    )));
    assert!(
        !retired
            .frontmatter
            .extension_keys()
            .contains(&"superseded_by")
    );

    assert!(with_frontmatter("type: X").frontmatter.is_current());
    assert!(
        with_frontmatter("type: X\nstatus: draft")
            .frontmatter
            .is_current()
    );
    // An unknown status is kept and shown, but it is not `stable`.
    let unknown = with_frontmatter("type: X\nstatus: experimental");
    assert_eq!(unknown.frontmatter.status().to_string(), "experimental");
    assert!(!unknown.frontmatter.is_current());
}

#[test]
fn a_rewrite_leaves_untouched_frontmatter_as_written() {
    // Flow style, both quote styles, an unquoted timestamp, a comment, an
    // extension key, and a nested extension key inside a defined mapping.
    let src = "\
---
type: Metric
# How it was made.
generated: { by: reference_agent/gemini-2.5-pro, at: \"2026-06-20T22:53:05Z\", x-acme-run: 42 }
verified:
- by: human:ahormati
  at: 2026-06-30
stale_after: '2026-12-31T00:00:00Z'
tags: [finance, revenue]
x-acme-owner: data-team   # team that owns it
---

# Revenue
";
    let mut doc = Document::parse(src).unwrap();
    // Nothing changed: the text comes back byte for byte.
    assert_eq!(doc.serialize(), src);

    // Change one value: only that entry is re-serialized.
    doc.frontmatter.set("title", Value::from("Revenue"));
    doc.frontmatter.set("tags", Value::from(vec!["finance"]));
    let out = doc.serialize();
    assert!(out.contains("# How it was made.\ngenerated: { by: reference_agent/gemini-2.5-pro, at: \"2026-06-20T22:53:05Z\", x-acme-run: 42 }\n"));
    assert!(out.contains("verified:\n- by: human:ahormati\n  at: 2026-06-30\n"));
    assert!(out.contains("stale_after: '2026-12-31T00:00:00Z'\n"));
    assert!(out.contains("x-acme-owner: data-team   # team that owns it\n"));
    assert!(out.contains("tags:\n  - finance\n"));
    assert!(out.ends_with("title: Revenue\n---\n\n# Revenue\n"));

    // Reordering moves the text without rewriting it, and removing a key
    // takes its comment with it.
    doc.frontmatter.remove("generated");
    doc.frontmatter.reorder_preferred();
    let reordered = doc.serialize();
    assert!(!reordered.contains("How it was made"));
    assert!(reordered.starts_with("---\ntype: Metric\ntitle: Revenue\ntags:\n  - finance\nverified:\n- by: human:ahormati\n  at: 2026-06-30\nstale_after: '2026-12-31T00:00:00Z'\n"));

    // The canonical form is still available, and is what `okf fmt` writes.
    let canonical = Document::parse(src).unwrap().serialize_canonical();
    assert!(canonical.contains("generated:\n  by: reference_agent/gemini-2.5-pro\n  at: \"2026-06-20T22:53:05Z\"\n  x-acme-run: 42\n"));
    assert_eq!(
        Document::parse(&canonical).unwrap().frontmatter,
        Document::parse(src).unwrap().frontmatter
    );
}

#[test]
fn a_byte_order_mark_and_crlf_do_not_hide_the_frontmatter() {
    let bom = Document::parse("\u{feff}---\ntype: X\n---\n\nBody.\n").unwrap();
    assert_eq!(bom.frontmatter.type_().as_deref(), Some("X"));
    assert_eq!(bom.body, "Body.");
    assert!(Document::has_frontmatter_block(
        "\u{feff}---\ntype: X\n---\n"
    ));

    let crlf = Document::parse("---\r\ntype: X\r\n---\r\n\r\nBody.\r\n").unwrap();
    assert_eq!(crlf.frontmatter.type_().as_deref(), Some("X"));
    assert_eq!(crlf.body, "Body.");

    assert!(!Document::has_frontmatter_block("# Just a heading\n"));
    assert!(Document::parse("---\ntype: X\n---\n").unwrap().is_concept());
    assert!(!Document::parse("# Just a heading\n").unwrap().is_concept());
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
    assert_eq!(doc.serialize(), "body\n");
}

#[test]
fn serialize_empty_frontmatter_omits_delimiters() {
    let no_fm = Document::parse("# Title\n\nSome body text.\n").unwrap();
    assert!(no_fm.frontmatter.is_empty());
    assert_eq!(no_fm.serialize(), "# Title\n\nSome body text.\n");

    let empty_fm = Document::parse("---\n---\n# Sub Title\n\nBody.\n").unwrap();
    assert!(empty_fm.frontmatter.is_empty());
    assert_eq!(empty_fm.serialize(), "# Sub Title\n\nBody.\n");

    let with_fm = Document::parse("---\ntype: Metric\n---\n\n# Metric\n\nBody.\n").unwrap();
    assert!(!with_fm.frontmatter.is_empty());
    assert!(
        with_fm
            .serialize()
            .starts_with("---\ntype: Metric\n---\n\n")
    );
}

#[test]
fn a_datetime_with_offset_is_valid_stale_after() {
    let doc = with_frontmatter("type: X\nstale_after: '2026-09-23T00:00:00Z'");
    let field = doc.frontmatter.stale_after().unwrap();
    assert!(field.is_valid());
    assert_eq!(
        field.datetime.unwrap().date,
        Date::new(2026, 9, 23).unwrap()
    );

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
    // Conventional headings carry no required behaviour, so this is the
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
    // The spec calls `type` "a short string", so `type: 42` is a producer
    // deviation, but the typed accessors coerce non-string scalars to their
    // display form the way the reference's `str(fm.get("type"))` does, rather
    // than returning `None` and leaving a `validate()`-passing concept
    // typeless. Locking this in so a "borrow-only" refactor does not silently
    // reintroduce the regression.
    let doc = with_frontmatter("type: 42\ntitle: hello");
    assert_eq!(doc.frontmatter.type_().as_deref(), Some("42"));
    // Non-string scalars remain permissive; the accessor surfaces what was
    // written, while non-scalar values are rejected by conformance validation.
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
    // A non-scalar (sequence or mapping) is not a plausible `type`, and the
    // document-level conformance check agrees with the type-based accessors.
    let sequence = with_frontmatter("type: [a, b]");
    assert!(sequence.frontmatter.type_().is_none());
    assert!(sequence.validate().is_err());

    let mapping = with_frontmatter("type: { name: Metric }");
    assert!(mapping.frontmatter.type_().is_none());
    assert!(mapping.validate().is_err());
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

#[test]
fn an_indented_line_of_dashes_does_not_end_the_frontmatter() {
    // The closing delimiter is `---` at the start of a line. Inside a block
    // scalar the same three dashes, indented, are text.
    let text = "---\ntype: Note\ndescription: |\n  Above the rule.\n  ---\n  Below the rule.\ntitle: Rules\n---\n\nBody.\n";
    let doc = Document::parse(text).unwrap();
    assert_eq!(
        doc.frontmatter.description().as_deref(),
        Some("Above the rule.\n---\nBelow the rule.\n")
    );
    assert_eq!(doc.frontmatter.title().as_deref(), Some("Rules"));
    assert_eq!(doc.body, "Body.");
    assert_eq!(doc.serialize(), text);
}
