//! YAML-subset parser/emitter tests, including the round-trip invariant.

use okf::yaml::Value;

fn roundtrip(src: &str) -> Value {
    let v = Value::parse(src).unwrap();
    let emitted = v.to_yaml_string();
    let reparsed = Value::parse(&emitted).unwrap();
    assert_eq!(
        v, reparsed,
        "round-trip mismatch.\nsrc:\n{src}\nemitted:\n{emitted}"
    );
    v
}

#[test]
fn scalars() {
    assert_eq!(
        Value::parse("hello").unwrap(),
        Value::String("hello".into())
    );
    assert_eq!(Value::parse("42").unwrap(), Value::Int(42));
    assert_eq!(Value::parse("-7").unwrap(), Value::Int(-7));
    assert_eq!(Value::parse("2.5").unwrap(), Value::Float(2.5));
    assert_eq!(Value::parse("true").unwrap(), Value::Bool(true));
    assert_eq!(Value::parse("false").unwrap(), Value::Bool(false));
    assert_eq!(Value::parse("null").unwrap(), Value::Null);
    assert_eq!(Value::parse("~").unwrap(), Value::Null);
    assert_eq!(Value::parse("").unwrap(), Value::Null);
}

#[test]
fn quoted_scalars() {
    assert_eq!(Value::parse("\"42\"").unwrap(), Value::String("42".into()));
    assert_eq!(
        Value::parse("'true'").unwrap(),
        Value::String("true".into())
    );
    assert_eq!(
        Value::parse("\"line1\\nline2\"").unwrap(),
        Value::String("line1\nline2".into())
    );
    assert_eq!(
        Value::parse(r#""\u263A""#).unwrap(),
        Value::String("\u{263A}".into())
    );
    assert_eq!(
        Value::parse("'it''s here'").unwrap(),
        Value::String("it's here".into())
    );
}

#[test]
fn malformed_quoted_scalars_are_errors() {
    for input in ["\"value\" trailing", "'value' trailing"] {
        assert!(
            Value::parse(input).is_err(),
            "accepted malformed input: {input:?}"
        );
    }

    assert!(Value::parse("\"value\" # comment").is_ok());
    assert!(Value::parse(r#""\q""#).is_err());
    assert!(Value::parse(r#""\u12""#).is_err());
    assert!(Value::parse(r#""\u12G4""#).is_err());
    assert!(Value::parse(r#""\uD800""#).is_err());
}

#[test]
fn dangling_flow_double_quoted_escape_is_an_error() {
    assert!(Value::parse("[\"dangling\\").is_err());
}

#[test]
fn block_mapping() {
    let v = roundtrip("type: BigQuery Table\ntitle: Orders\ncount: 3\n");
    let m = v.as_mapping().unwrap();
    assert_eq!(m.get("type").unwrap().as_str(), Some("BigQuery Table"));
    assert_eq!(m.get("count").unwrap().as_int(), Some(3));
    // Key order is preserved.
    assert_eq!(m.keys().collect::<Vec<_>>(), vec!["type", "title", "count"]);
}

#[test]
fn flow_and_block_sequences() {
    let flow = roundtrip("tags: [sales, orders, revenue]\n");
    assert_eq!(
        flow.as_mapping()
            .unwrap()
            .get("tags")
            .unwrap()
            .as_sequence()
            .unwrap()
            .len(),
        3
    );
    let block = roundtrip("tags:\n  - sales\n  - orders\n");
    let tags = block.as_mapping().unwrap().get("tags").unwrap();
    assert_eq!(tags.as_sequence().unwrap()[0].as_str(), Some("sales"));
}

#[test]
fn nested_mappings() {
    roundtrip("a:\n  b:\n    c: deep\n  d: 2\ne: top\n");
}

#[test]
fn flow_mapping() {
    let v = roundtrip("obj: {x: 1, y: two}\n");
    let obj = v
        .as_mapping()
        .unwrap()
        .get("obj")
        .unwrap()
        .as_mapping()
        .unwrap();
    assert_eq!(obj.get("x").unwrap().as_int(), Some(1));
    assert_eq!(obj.get("y").unwrap().as_str(), Some("two"));
}

#[test]
fn colons_inside_flow_scalars_are_content() {
    // OKF v0.2 frontmatter depends on this: `human:ahormati` and an ISO-8601
    // time both carry colons that do not separate a key from a value (§5.2, §7).
    let v =
        roundtrip("generated: { by: reference_agent/gemini-2.5-pro, at: 2026-06-20T22:53:05Z }\n");
    let generated = v
        .as_mapping()
        .unwrap()
        .get("generated")
        .unwrap()
        .as_mapping()
        .unwrap();
    assert_eq!(
        generated.get("by").unwrap().as_str(),
        Some("reference_agent/gemini-2.5-pro")
    );
    assert_eq!(
        generated.get("at").unwrap().as_str(),
        Some("2026-06-20T22:53:05Z")
    );

    let verified = roundtrip("verified: { by: human:ahormati, at: 2026-06-25T09:00:00Z }\n");
    let by = verified
        .as_mapping()
        .unwrap()
        .get("verified")
        .unwrap()
        .as_mapping()
        .unwrap();
    assert_eq!(by.get("by").unwrap().as_str(), Some("human:ahormati"));

    // A URL in a flow scalar survives too.
    let url = Value::parse("{ resource: https://wiki.acme/finance/revenue-recognition }").unwrap();
    assert_eq!(
        url.as_mapping().unwrap().get("resource").unwrap().as_str(),
        Some("https://wiki.acme/finance/revenue-recognition")
    );
}

#[test]
fn block_sequence_of_flow_mappings() {
    let v = roundtrip(
        "parameters:\n  - { name: year, type: integer, required: true }\n  - { name: segment, type: string, required: true }\n",
    );
    let params = v
        .as_mapping()
        .unwrap()
        .get("parameters")
        .unwrap()
        .as_sequence()
        .unwrap();
    assert_eq!(params.len(), 2);
    let first = params[0].as_mapping().unwrap();
    assert_eq!(first.get("name").unwrap().as_str(), Some("year"));
    assert_eq!(first.get("type").unwrap().as_str(), Some("integer"));
    assert_eq!(first.get("required").unwrap().as_bool(), Some(true));
}

#[test]
fn sequences_of_mappings_emit_idiomatic_bullets() {
    let v = Value::parse(
        "parameters:\n  - { name: year, type: integer }\n  - { name: segment, type: string }\n",
    )
    .unwrap();
    let emitted = v.to_yaml_string();
    assert_eq!(
        emitted,
        "parameters:\n  - name: year\n    type: integer\n  - name: segment\n    type: string\n"
    );
    assert_eq!(Value::parse(&emitted).unwrap(), v);
}

#[test]
#[allow(clippy::literal_string_with_formatting_args)] // YAML test data, not a format string
fn flow_key_without_a_value_is_null() {
    // YAML reads `{a:1}` as one key with no value, since the colon is not
    // followed by whitespace.
    let v = Value::parse("{a:1}").unwrap();
    let m = v.as_mapping().unwrap();
    assert_eq!(m.keys().collect::<Vec<_>>(), vec!["a:1"]);
    assert_eq!(m.get("a:1"), Some(&Value::Null));
}

#[test]
fn comments_are_ignored() {
    let v = Value::parse("# leading comment\ntype: X  # trailing\ntitle: Y\n").unwrap();
    let m = v.as_mapping().unwrap();
    assert_eq!(m.get("type").unwrap().as_str(), Some("X"));
    assert_eq!(m.get("title").unwrap().as_str(), Some("Y"));
}

#[test]
fn literal_block_scalar() {
    let v = Value::parse("body: |\n  line one\n  line two\n").unwrap();
    assert_eq!(
        v.as_mapping().unwrap().get("body").unwrap().as_str(),
        Some("line one\nline two\n")
    );
}

#[test]
fn folded_block_scalar() {
    let v = Value::parse("body: >\n  line one\n  line two\n").unwrap();
    assert_eq!(
        v.as_mapping().unwrap().get("body").unwrap().as_str(),
        Some("line one line two\n")
    );
}

#[test]
fn strings_needing_quotes_roundtrip() {
    // A string that looks like a number / bool / has special chars must be
    // quoted on emit so it re-parses as a string.
    for s in ["42", "true", "null", "a: b", "value # x", "", "  spaced  "] {
        let v = Value::String(s.to_string());
        let emitted = Value::Mapping({
            let mut m = okf::yaml::Mapping::new();
            m.insert("k", v.clone());
            m
        })
        .to_yaml_string();
        let reparsed = Value::parse(&emitted).unwrap();
        assert_eq!(
            reparsed.as_mapping().unwrap().get("k"),
            Some(&v),
            "string {s:?} did not round-trip; emitted: {emitted}"
        );
    }
}

#[test]
fn block_sequence_at_parent_indent() {
    // This is exactly what PyYAML's safe_dump (the reference serializer) emits
    // for list values: dashes at the same column as the key.
    let v = Value::parse("type: X\ntags:\n- sales\n- orders\ntitle: Y\n").unwrap();
    let m = v.as_mapping().unwrap();
    let tags = m.get("tags").unwrap().as_sequence().unwrap();
    assert_eq!(tags.len(), 2);
    assert_eq!(tags[0].as_str(), Some("sales"));
    assert_eq!(m.get("title").unwrap().as_str(), Some("Y"));
    // And nested under a deeper mapping.
    let nested = Value::parse("outer:\n  tags:\n  - a\n  - b\n").unwrap();
    let inner = nested
        .as_mapping()
        .unwrap()
        .get("outer")
        .unwrap()
        .as_mapping()
        .unwrap();
    assert_eq!(inner.get("tags").unwrap().as_sequence().unwrap().len(), 2);
}

#[test]
fn conservative_number_resolution() {
    // Zero-padded codes stay strings (not coerced to ints).
    assert_eq!(Value::parse("007").unwrap(), Value::String("007".into()));
    assert_eq!(Value::parse("08").unwrap(), Value::String("08".into()));
    // Bare-exponent forms stay strings; only point-bearing floats are floats.
    assert_eq!(Value::parse("1e3").unwrap(), Value::String("1e3".into()));
    assert_eq!(Value::parse("1.5e3").unwrap(), Value::Float(1500.0));
    assert_eq!(Value::parse("0").unwrap(), Value::Int(0));
    assert_eq!(Value::parse("-42").unwrap(), Value::Int(-42));
}

#[test]
fn non_finite_and_large_floats_roundtrip() {
    for f in [f64::INFINITY, f64::NEG_INFINITY, 1e30, -2.5e-12, 1.0] {
        let v = Value::Float(f);
        let mut m = okf::yaml::Mapping::new();
        m.insert("k", v.clone());
        let emitted = Value::Mapping(m).to_yaml_string();
        let reparsed = Value::parse(&emitted).unwrap();
        let got = reparsed.as_mapping().unwrap().get("k").unwrap();
        match got {
            Value::Float(g) => assert_eq!(g.to_bits(), f.to_bits(), "emitted: {emitted}"),
            other => panic!("{f} round-tripped as {other:?} (emitted: {emitted})"),
        }
    }
    // NaN is a float on the way back (compared specially).
    let mut m = okf::yaml::Mapping::new();
    m.insert("k", Value::Float(f64::NAN));
    let reparsed = Value::parse(&Value::Mapping(m).to_yaml_string()).unwrap();
    assert!(matches!(reparsed.as_mapping().unwrap().get("k"), Some(Value::Float(g)) if g.is_nan()));
}

#[test]
fn unterminated_flow_is_error() {
    assert!(Value::parse("tags: [a, b").is_err());
}

#[test]
fn tab_indentation_is_error() {
    assert!(Value::parse("a:\n\tb: 1").is_err());
}

/// Frontmatter exactly as `PyYAML`'s `safe_dump` writes it, which is what the
/// reference implementation publishes. Every wrapping and quoting decision here
/// is `PyYAML`'s, not ours.
const PYYAML_DUMPED: &str = "\
type: BigQuery Table
title: GA4 Events Export
description: Google Analytics 4 event-level daily sharded export tables containing
  user interaction logs.
tags:
- analytics
- e-commerce
generated:
  by: reference_agent/gemini-3.5-flash
  at: '2026-07-10T21:15:20+00:00'
sources:
- title: 'Google Analytics Help: BigQuery Export Schema'
  id: ga4-export-docs
  resource: https://support.google.com/analytics/answer/7029846
";

#[test]
fn pyyaml_wrapped_plain_scalars_fold_into_one_value() {
    // `safe_dump` wraps anything past its 80-column width onto a continuation
    // line, and YAML folds that break into a single space. A parser that treats
    // the continuation as an indentation error cannot read reference-produced
    // bundles at all.
    let v = roundtrip(PYYAML_DUMPED);
    let m = v.as_mapping().unwrap();
    assert_eq!(
        m.get("description").unwrap().as_str(),
        Some(
            "Google Analytics 4 event-level daily sharded export tables containing \
             user interaction logs."
        )
    );
    assert_eq!(m.get("tags").unwrap().as_sequence().unwrap().len(), 2);

    let generated = m.get("generated").unwrap().as_mapping().unwrap();
    assert_eq!(
        generated.get("at").unwrap().as_str(),
        Some("2026-07-10T21:15:20+00:00")
    );

    // A quoted scalar containing a `: ` stays one scalar, not a nested mapping.
    let source = m.get("sources").unwrap().as_sequence().unwrap()[0]
        .as_mapping()
        .unwrap();
    assert_eq!(
        source.get("title").unwrap().as_str(),
        Some("Google Analytics Help: BigQuery Export Schema")
    );
}

#[test]
fn a_wrapped_quoted_scalar_folds_even_across_a_key_shaped_line() {
    // Inside an unclosed quote every line belongs to the scalar, so a wrapped
    // value that happens to break before `Something: else` is not misread as a
    // mapping entry.
    let v =
        Value::parse("title: 'Database schema documentation and\n  SEDE: the sequel'\n").unwrap();
    assert_eq!(
        v.as_mapping().unwrap().get("title").unwrap().as_str(),
        Some("Database schema documentation and SEDE: the sequel")
    );
}

#[test]
fn a_multi_line_plain_sequence_item_folds() {
    let v = Value::parse("tags:\n- a tag that wraps\n  onto a second line\n- short\n").unwrap();
    let tags = v
        .as_mapping()
        .unwrap()
        .get("tags")
        .unwrap()
        .as_sequence()
        .unwrap();
    assert_eq!(
        tags[0].as_str(),
        Some("a tag that wraps onto a second line")
    );
    assert_eq!(tags[1].as_str(), Some("short"));
}

#[test]
fn iso_datetimes_emit_quoted_but_bare_dates_stay_plain() {
    // PyYAML types a bare ISO datetime as a `datetime` and re-dumps it as
    // `2026-06-30 14:00:00+00:00`, losing the `T` and `Z` the spec asks for. A
    // quoted timestamp survives that round-trip byte-identical, so emit one.
    let mut m = okf::yaml::Mapping::new();
    m.insert("at", Value::String("2026-06-30T14:00:00Z".into()));
    m.insert("stale_after", Value::String("2026-12-31".into()));
    m.insert("offset", Value::String("2026-07-10T21:15:20+00:00".into()));
    let emitted = Value::Mapping(m).to_yaml_string();

    assert!(
        emitted.contains("at: \"2026-06-30T14:00:00Z\""),
        "{emitted}"
    );
    assert!(
        emitted.contains("offset: \"2026-07-10T21:15:20+00:00\""),
        "{emitted}"
    );
    // A date carries no time, so it stays plain: that is how the spec and the
    // reference both write `stale_after`, `last_modified`, and `usage_window`.
    assert!(emitted.contains("stale_after: 2026-12-31"), "{emitted}");

    // Either spelling still reads back as the same string for this crate.
    let reparsed = Value::parse(&emitted).unwrap();
    let back = reparsed.as_mapping().unwrap();
    assert_eq!(
        back.get("at").unwrap().as_str(),
        Some("2026-06-30T14:00:00Z")
    );
    assert_eq!(
        back.get("stale_after").unwrap().as_str(),
        Some("2026-12-31")
    );
}
