//! Bundle loading, the cross-link graph, and conformance, exercised against the
//! spec's Appendix A minimal example bundle.

mod common;

use common::TempDir;
use okf_core::{
    Bundle, BundleError, ConceptId, ConceptIdError, DocumentError, Severity, validate_bundle,
};

/// Builds the Appendix A example bundle and returns its temp dir.
fn appendix_a() -> TempDir {
    let tmp = TempDir::new();
    tmp.write(
        "datasets/sales.md",
        "---\n\
         type: BigQuery Dataset\n\
         title: Sales\n\
         description: All sales-related tables for the retail business.\n\
         resource: https://console.cloud.google.com/bigquery?p=acme&d=sales\n\
         tags: [sales]\n\
         timestamp: 2026-05-28T00:00:00Z\n\
         ---\n\n\
         The sales dataset contains transactional tables, including\n\
         [orders](/tables/orders.md) and [customers](/tables/customers.md).\n",
    );
    tmp.write(
        "tables/orders.md",
        "---\n\
         type: BigQuery Table\n\
         title: Orders\n\
         description: One row per completed customer order.\n\
         resource: https://console.cloud.google.com/bigquery?p=acme&d=sales&t=orders\n\
         tags: [sales, orders]\n\
         timestamp: 2026-05-28T00:00:00Z\n\
         ---\n\n\
         # Schema\n\n\
         Part of the [sales dataset](/datasets/sales.md). FK to [customers](/tables/customers.md).\n",
    );
    tmp.write(
        "tables/customers.md",
        "---\n\
         type: BigQuery Table\n\
         title: Customers\n\
         description: One row per customer.\n\
         timestamp: 2026-05-28T00:00:00Z\n\
         ---\n\n\
         Linked from [orders](/tables/orders.md).\n",
    );
    tmp
}

#[test]
fn loads_all_concepts() {
    let tmp = appendix_a();
    let bundle = Bundle::load(tmp.path()).unwrap();
    assert_eq!(bundle.len(), 3);
    assert!(bundle.contains(&ConceptId::parse("tables/orders").unwrap()));
    assert!(bundle.contains(&ConceptId::parse("datasets/sales").unwrap()));
    assert!(bundle.parse_errors().is_empty());
}

#[test]
fn resolves_cross_links_and_backlinks() {
    let tmp = appendix_a();
    let bundle = Bundle::load(tmp.path()).unwrap();

    let sales = ConceptId::parse("datasets/sales").unwrap();
    let orders = ConceptId::parse("tables/orders").unwrap();
    let customers = ConceptId::parse("tables/customers").unwrap();

    let sales_links: Vec<_> = bundle
        .links_from(&sales)
        .iter()
        .map(|l| l.target.clone())
        .collect();
    assert!(sales_links.contains(&orders));
    assert!(sales_links.contains(&customers));
    assert!(bundle.links_from(&sales).iter().all(|l| l.exists));

    // orders is linked from sales and customers.
    let backlinks = bundle.backlinks(&orders);
    assert!(backlinks.contains(&sales));
    assert!(backlinks.contains(&customers));

    assert!(bundle.broken_links().is_empty());
}

#[test]
fn broken_links_are_detected_but_not_fatal() {
    let tmp = TempDir::new();
    tmp.write(
        "a.md",
        "---\ntype: Note\n---\nSee [missing](/does/not/exist.md).\n",
    );
    let bundle = Bundle::load(tmp.path()).unwrap();
    let broken = bundle.broken_links();
    assert_eq!(broken.len(), 1);
    assert_eq!(broken[0].1, "/does/not/exist.md");

    // Broken links are informational, not conformance errors.
    let report = validate_bundle(&bundle);
    assert!(report.is_conformant());
    assert!(
        report
            .of(Severity::Info)
            .any(|d| d.message.contains("does/not/exist"))
    );
}

#[test]
fn appendix_a_is_conformant() {
    let tmp = appendix_a();
    let bundle = Bundle::load(tmp.path()).unwrap();
    let report = validate_bundle(&bundle);
    assert!(report.is_conformant(), "{:#?}", report.diagnostics);
    assert_eq!(report.error_count(), 0);
}

#[test]
fn missing_type_is_a_conformance_error() {
    let tmp = TempDir::new();
    tmp.write("bad.md", "---\ntitle: No Type\n---\nbody\n");
    let bundle = Bundle::load(tmp.path()).unwrap();
    let report = validate_bundle(&bundle);
    assert!(!report.is_conformant());
    assert!(
        report
            .of(Severity::Error)
            .any(|d| d.message.contains("type"))
    );
}

#[test]
fn reserved_files_are_recognized_not_concepts() {
    let tmp = TempDir::new();
    tmp.write("a.md", "---\ntype: Note\n---\nbody\n");
    tmp.write("index.md", "# Listing\n\n* [a](a.md)\n");
    tmp.write(
        "log.md",
        "# Log\n\n## 2026-05-22\n* **Update**: did a thing.\n",
    );
    let bundle = Bundle::load(tmp.path()).unwrap();
    assert_eq!(bundle.len(), 1); // only a.md is a concept
    assert_eq!(bundle.index_files().len(), 1);
    assert_eq!(bundle.log_files().len(), 1);
}

#[test]
fn okf_version_read_from_root_index() {
    let tmp = TempDir::new();
    tmp.write("a.md", "---\ntype: Note\n---\nbody\n");
    tmp.write("index.md", "---\nokf_version: \"0.1\"\n---\n\n# Listing\n");
    let bundle = Bundle::load(tmp.path()).unwrap();
    assert_eq!(bundle.okf_version(), Some("0.1"));
}

#[test]
fn a_filename_that_is_not_a_valid_concept_id_segment_still_loads() {
    // §11 makes conformance a question of frontmatter, not filenames, and the
    // reference's `path_to_concept_id` validates nothing. So a readable document
    // under an awkward name is a concept, not a parse error.
    let tmp = TempDir::new();
    tmp.write(
        "tables/my notes.md",
        "---\ntype: Reference\ntitle: My notes\ndescription: Scratch notes.\n---\n\nProse.\n",
    );
    let bundle = Bundle::load(tmp.path()).unwrap();

    assert!(
        bundle.parse_errors().is_empty(),
        "{:?}",
        bundle.parse_errors()
    );
    assert_eq!(bundle.len(), 1);
    let id = &bundle.concepts()[0].id;
    assert_eq!(id.to_string(), "tables/my notes");

    // The id the loader produced round-trips, so a consumer can look the
    // concept back up by the id it was handed.
    assert_eq!(ConceptId::parse(&id.to_string()).as_ref(), Ok(id));
    assert!(bundle.contains(&ConceptId::parse("tables/my notes").unwrap()));

    // Still conformant: the name is only worth a warning, since §11 makes
    // conformance a question of frontmatter.
    let report = validate_bundle(&bundle);
    assert!(report.is_conformant());
    assert!(
        report
            .of(Severity::Warning)
            .any(|d| d.message.contains("my notes")),
        "{:?}",
        report.diagnostics
    );
}

#[test]
fn links_to_names_with_spaces_and_emoji_join_the_graph() {
    // Before the segment rule was relaxed these links resolved to nothing and
    // were not even reported as broken: the edge vanished silently.
    let tmp = TempDir::new();
    tmp.write(
        "tables/my notes.md",
        "---\ntype: Reference\ntitle: Notes\ndescription: d\n---\n\nProse.\n",
    );
    tmp.write(
        "tables/rocket\u{1f680}.md",
        "---\ntype: Reference\ntitle: Rocket\ndescription: d\n---\n\nProse.\n",
    );
    tmp.write(
        "tables/orders.md",
        "---\ntype: Reference\ntitle: Orders\ndescription: d\n---\n\n\
         Plain [a](/tables/my notes.md), bracketed [b](</tables/my notes.md>),\n\
         encoded [c](/tables/my%20notes.md), emoji [d](/tables/rocket\u{1f680}.md).\n",
    );

    let bundle = Bundle::load(tmp.path()).unwrap();
    let orders = ConceptId::parse("tables/orders").unwrap();
    let notes = ConceptId::parse("tables/my notes").unwrap();
    let rocket = ConceptId::parse("tables/rocket\u{1f680}").unwrap();

    let resolved = bundle.links_from(&orders);
    assert_eq!(resolved.len(), 4);
    assert!(
        resolved.iter().all(|l| l.exists),
        "unresolved: {:?}",
        resolved.iter().filter(|l| !l.exists).collect::<Vec<_>>()
    );
    assert_eq!(
        resolved.iter().filter(|l| l.target == notes).count(),
        3,
        "all three spellings should name the same concept"
    );
    assert_eq!(bundle.backlinks(&notes), std::slice::from_ref(&orders));
    assert_eq!(bundle.backlinks(&rocket), std::slice::from_ref(&orders));
    assert!(bundle.broken_links().is_empty());
}

#[test]
fn a_broken_link_to_a_spaced_name_is_still_reported() {
    let tmp = TempDir::new();
    tmp.write(
        "a.md",
        "---\ntype: Reference\ntitle: A\ndescription: d\n---\n\n[gone](/tables/no such.md)\n",
    );
    let bundle = Bundle::load(tmp.path()).unwrap();
    let broken = bundle.broken_links();
    assert_eq!(broken.len(), 1);
    assert_eq!(broken[0].1, "/tables/no such.md");
}

#[test]
fn an_awkward_segment_warns_once_not_once_per_file() {
    let tmp = TempDir::new();
    for name in ["a", "b", "c"] {
        tmp.write(
            &format!("my dir/{name}.md"),
            "---\ntype: Reference\ntitle: T\ndescription: d\n---\n\nProse.\n",
        );
    }
    let bundle = Bundle::load(tmp.path()).unwrap();
    let report = validate_bundle(&bundle);
    assert!(report.is_conformant());
    let warnings: Vec<_> = report
        .of(Severity::Warning)
        .filter(|d| d.message.contains("my dir"))
        .collect();
    assert_eq!(warnings.len(), 1, "{warnings:?}");
}

#[test]
fn parallel_load_path_preserves_sorted_concept_order() {
    // The parallel loader only kicks in past `PARALLEL_THRESHOLD` files, so
    // build a bundle with enough concepts to exercise it and verify the
    // merged result is still in deterministic sorted order with the same
    // graph as a sequential load would produce.
    let tmp = TempDir::new();
    let names = [
        "alpha", "bravo", "charlie", "delta", "echo", "foxtrot", "golf", "hotel", "india",
        "juliet", "kilo", "lima", "mike",
    ];
    for name in names {
        tmp.write(
            &format!("{name}.md"),
            &format!("---\ntype: Reference\ntitle: {name}\ndescription: d\n---\n\nProse.\n"),
        );
    }
    let bundle = Bundle::load(tmp.path()).unwrap();
    assert_eq!(bundle.len(), names.len());
    assert!(bundle.parse_errors().is_empty());

    // `concepts` is documented to be in path order; the parallel merge keeps
    // the input chunk order, so the ids come out sorted lexicographically.
    let ids: Vec<String> = bundle.concepts().iter().map(|c| c.id.to_string()).collect();
    let mut expected = names.to_vec();
    expected.sort_unstable();
    let expected: Vec<String> = expected.iter().map(|s| s.to_string()).collect();
    assert_eq!(ids, expected);
}

#[test]
fn parallel_load_surfaces_io_errors_in_file_order() {
    // >PARALLEL_THRESHOLD files so the worker-thread path is taken. A
    // mid-list file is non-UTF8, so `read_to_string` returns an `io::Error`,
    // and the loader surfaces the first I/O failure in chunk order — which,
    // because chunks are contiguous slices of the sorted file list, is the
    // earliest failure in file order.
    let tmp = TempDir::new();
    let names = [
        "a", "b", "c", "d", "e", "f", "g", "h", "i", "j", "k", "l", "m",
    ];
    for name in names {
        tmp.write(
            &format!("{name}.md"),
            &format!("---\ntype: Reference\ntitle: {name}\ndescription: d\n---\n\nProse.\n"),
        );
    }
    // Overwrite `f.md` with invalid UTF-8 (`read_to_string` errors here).
    std::fs::write(tmp.path().join("f.md"), b"\xff\xfe not utf-8").unwrap();

    let err = Bundle::load(tmp.path()).expect_err("non-UTF8 file must error");
    let BundleError::Io { kind, .. } = err else {
        panic!("expected BundleError::Io, got {err:?}");
    };
    // `read_to_string` reports `InvalidData` for non-UTF8 input.
    assert_eq!(kind, std::io::ErrorKind::InvalidData);
}

#[test]
fn document_error_invalid_concept_id_carries_a_typed_source() {
    // The loader stores concept-id failures as `DocumentError::InvalidConceptId`,
    // preserving the typed `ConceptIdError` through both `From` and `source()`.
    // A directory-as-file produces a path that `ConceptId::from_path` rejects
    // (the path is still under root, but `from_path` strips `.md` from the
    // stem only if present, leaving a directory name that fails segment
    // validation when it contains a path separator).
    let tmp = TempDir::new();
    tmp.write(
        "good.md",
        "---\ntype: Reference\ntitle: Good\ndescription: d\n---\n\nProse.\n",
    );
    // A file whose name fails segment validation. `..` is rejected by
    // `validate_segment`; placed at the bundle root, `from_path` yields a
    // one-segment id of `..`, which `ConceptId::new` would reject — but
    // `from_path` does not validate, so the rejection has to come from
    // elsewhere. Use a path that is genuinely not under the bundle root by
    // passing a root that does not contain it.
    let outside = TempDir::new();
    std::fs::write(outside.path().join("stray.md"), "no frontmatter").unwrap();
    // `Bundle::load` is given `tmp` as root, but we point a real `from_path`
    // call at the stray file to drive the typed-error path directly.
    let stray = outside.path().join("stray.md");
    let err = ConceptId::from_path(tmp.path(), &stray).expect_err("path outside root must error");
    let doc_err: DocumentError = err.into();
    assert!(
        matches!(doc_err, DocumentError::InvalidConceptId(_)),
        "got {doc_err:?}"
    );
    // `source()` returns the inner `ConceptIdError`, so the typed cause
    // survives the conversion.
    let src = std::error::Error::source(&doc_err)
        .and_then(|s| s.downcast_ref::<ConceptIdError>())
        .expect("source should be a ConceptIdError");
    let _ = src; // presence is the assertion
}
