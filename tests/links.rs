//! Link classification, resolution, and citation extraction tests (§5, §8).

use okf::links::{extract_citations, extract_links, Link, LinkKind};
use okf::{ConceptId, Document};

#[test]
fn classify_link_kinds() {
    assert_eq!(Link::classify("/tables/users.md"), LinkKind::Absolute);
    assert_eq!(Link::classify("./other.md"), LinkKind::Relative);
    assert_eq!(Link::classify("../sibling.md"), LinkKind::Relative);
    assert_eq!(Link::classify("https://example.com"), LinkKind::External);
    assert_eq!(Link::classify("mailto:a@b.com"), LinkKind::External);
    assert_eq!(Link::classify("#section"), LinkKind::Anchor);
}

#[test]
fn extract_inline_links() {
    let body = "See [customers](/tables/customers.md) and [docs](https://example.com \"title\").";
    let links = extract_links(body);
    assert_eq!(links.len(), 2);
    assert_eq!(links[0].text, "customers");
    assert_eq!(links[0].target, "/tables/customers.md");
    assert_eq!(links[0].kind, LinkKind::Absolute);
    // Title stripped from the second link.
    assert_eq!(links[1].target, "https://example.com");
}

#[test]
fn links_inside_code_are_ignored() {
    let body = "Real [a](/a.md).\n\n```\nNot a [link](/b.md) in code.\n```\n\nInline `[c](/c.md)` ignored.\n";
    let links = extract_links(body);
    let targets: Vec<_> = links.iter().map(|l| l.target.as_str()).collect();
    assert_eq!(targets, vec!["/a.md"]);
}

#[test]
fn resolve_absolute_link() {
    let source = ConceptId::parse("tables/orders").unwrap();
    let link = Link {
        text: "customers".into(),
        target: "/tables/customers.md".into(),
        kind: LinkKind::Absolute,
    };
    assert_eq!(
        link.resolve(&source),
        Some(ConceptId::parse("tables/customers").unwrap())
    );
}

#[test]
fn resolve_relative_link() {
    let source = ConceptId::parse("tables/orders").unwrap();
    let link = Link {
        text: "neighbor".into(),
        target: "./customers.md".into(),
        kind: LinkKind::Relative,
    };
    assert_eq!(
        link.resolve(&source),
        Some(ConceptId::parse("tables/customers").unwrap())
    );

    let up = Link {
        text: "up".into(),
        target: "../datasets/sales.md".into(),
        kind: LinkKind::Relative,
    };
    assert_eq!(
        up.resolve(&source),
        Some(ConceptId::parse("datasets/sales").unwrap())
    );
}

#[test]
fn protocol_relative_url_is_external() {
    assert_eq!(Link::classify("//cdn.example.com/x.js"), LinkKind::External);
}

#[test]
fn any_uri_scheme_is_external() {
    // §4.1's `resource` is a URI, not necessarily an http URL, so a warehouse
    // identifier must not be mistaken for a relative path.
    assert_eq!(Link::classify("bigquery:acme.sales.orders"), LinkKind::External);
    assert_eq!(Link::classify("urn:acme:metric:revenue"), LinkKind::External);
    assert_eq!(Link::classify("s3://bucket/key"), LinkKind::External);
    // A path with a colon later on is still a path.
    assert_eq!(Link::classify("./notes/a:b.md"), LinkKind::Relative);
    assert_eq!(Link::classify("/tables/orders.md"), LinkKind::Absolute);
    // A leading digit is not a valid scheme.
    assert_eq!(Link::classify("2026:notes.md"), LinkKind::Relative);
}

#[test]
fn field_paths_try_both_readings_of_a_relative_target() {
    // §6.2 reads a relative path against the concept; §6.3's `references/`
    // convention is written from the bundle root. Both are offered, in that
    // order.
    let from = ConceptId::parse("computations/revenue").unwrap();
    assert_eq!(
        okf::links::field_path_candidates("references/skills/run-on-bq.md", &from),
        vec![
            "computations/references/skills/run-on-bq.md".to_string(),
            "references/skills/run-on-bq.md".to_string(),
        ]
    );
    // An absolute path has exactly one reading.
    assert_eq!(
        okf::links::field_path_candidates("/references/x.py", &from),
        vec!["references/x.py".to_string()]
    );
    // A URI has none.
    assert!(okf::links::field_path_candidates("https://example.com/x", &from).is_empty());
}

#[test]
fn absolute_link_normalizes_dot_segments() {
    let source = ConceptId::parse("a/b").unwrap();
    let link = Link {
        text: "x".into(),
        target: "/tables/../datasets/sales.md".into(),
        kind: LinkKind::Absolute,
    };
    assert_eq!(
        link.resolve(&source),
        Some(ConceptId::parse("datasets/sales").unwrap())
    );
}

#[test]
fn external_links_do_not_resolve() {
    let source = ConceptId::parse("a").unwrap();
    let link = Link {
        text: "x".into(),
        target: "https://example.com".into(),
        kind: LinkKind::External,
    };
    assert_eq!(link.resolve(&source), None);
}

#[test]
fn citations_section_parsed() {
    let body = "Prose.\n\n# Citations\n\n[1] [BigQuery schema](https://bq.example/schema)\n[2] [Runbook](https://wiki.acme.internal/runbook)\n";
    let citations = extract_citations(body);
    assert_eq!(citations.len(), 2);
    assert_eq!(citations[0].number, 1);
    assert_eq!(citations[0].text.as_deref(), Some("BigQuery schema"));
    assert_eq!(citations[0].target.as_deref(), Some("https://bq.example/schema"));
    assert_eq!(citations[1].number, 2);
}

#[test]
fn citations_stop_at_next_heading() {
    let body = "# Citations\n[1] [a](https://a)\n\n# Other\n[2] [b](https://b)\n";
    let citations = extract_citations(body);
    assert_eq!(citations.len(), 1);
}

#[test]
fn document_links_and_citations_integration() {
    let doc = Document::parse(
        "---\ntype: BigQuery Table\n---\n\nJoined with [customers](/tables/customers.md).\n\n# Citations\n[1] [BQ](https://bq)\n",
    )
    .unwrap();
    // links() returns every body link, including the one in the citation list.
    assert_eq!(doc.links().len(), 2);
    let internal: Vec<_> = doc.links().into_iter().filter(|l| l.kind == LinkKind::Absolute).collect();
    assert_eq!(internal.len(), 1);
    assert_eq!(doc.citations().len(), 1);
}
