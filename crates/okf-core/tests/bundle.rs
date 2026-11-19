//! Bundle loading, the cross-link graph, and conformance, exercised against the
//! spec's Appendix A minimal example bundle.

mod common;

use common::TempDir;
use okf_core::{
    Bundle, ConceptId, ConceptIdError, DocumentError, LinkTarget, NonConceptReason, PathResolution,
};
use okf_validator::{Severity, validate_bundle};

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
fn a_file_without_type_is_not_a_concept() {
    // A markdown file with no frontmatter, or no `type`, is not a concept.
    // It is skipped, and the bundle stays conformant: this is what lets a
    // bundle hold a README.
    let tmp = TempDir::new();
    tmp.write("a.md", "---\ntype: Note\n---\nbody\n");
    tmp.write("untyped.md", "---\ntitle: No Type\n---\nbody\n");
    tmp.write("blank-type.md", "---\ntype: \"\"\n---\nbody\n");
    tmp.write("README.md", "# Read me\n\nNot knowledge.\n");
    let bundle = Bundle::load(tmp.path()).unwrap();

    assert_eq!(bundle.len(), 1);
    assert!(bundle.parse_errors().is_empty());
    let skipped: Vec<(String, NonConceptReason)> = bundle
        .non_concepts()
        .iter()
        .map(|file| {
            (
                file.path
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned(),
                file.reason,
            )
        })
        .collect();
    assert_eq!(
        skipped,
        vec![
            ("README.md".to_string(), NonConceptReason::NoFrontmatter),
            ("blank-type.md".to_string(), NonConceptReason::NoType),
            ("untyped.md".to_string(), NonConceptReason::NoType),
        ]
    );

    let report = validate_bundle(&bundle);
    assert!(report.is_conformant());
    assert_eq!(
        report
            .of(Severity::Warning)
            .filter(|d| d.message.contains("not a concept"))
            .count(),
        3
    );
}

#[test]
fn dot_paths_are_not_part_of_the_bundle() {
    let tmp = TempDir::new();
    tmp.write("a.md", "---\ntype: Note\n---\nbody\n");
    tmp.write(".hidden.md", "---\ntype: Note\n---\nbody\n");
    tmp.write(
        ".github/pull_request_template.md",
        "---\nnot: [valid\n---\n",
    );
    tmp.write("tables/.drafts/wip.md", "---\ntype: Note\n---\nbody\n");
    let bundle = Bundle::load(tmp.path()).unwrap();
    assert_eq!(bundle.len(), 1);
    assert!(bundle.non_concepts().is_empty());
    // Not even its broken frontmatter is looked at.
    assert!(bundle.parse_errors().is_empty());
}

#[cfg(unix)]
#[test]
fn symbolic_links_out_of_the_bundle_root_are_not_followed() {
    let outside = TempDir::new();
    outside.write("secret.md", "---\ntype: Secret\n---\nbody\n");
    outside.write("attester.py", "print('x')\n");

    let tmp = TempDir::new();
    tmp.write(
        "a.md",
        "---\ntype: Attested Computation\nruntime: python\nattester:\n  resource: /escape.py\n---\n\
         See [inside](inside.md) and [outside](outside.md).\n",
    );
    tmp.write("real.md", "---\ntype: Note\n---\nbody\n");
    std::os::unix::fs::symlink(tmp.path().join("real.md"), tmp.path().join("inside.md")).unwrap();
    std::os::unix::fs::symlink(
        outside.path().join("secret.md"),
        tmp.path().join("outside.md"),
    )
    .unwrap();
    std::os::unix::fs::symlink(
        outside.path().join("attester.py"),
        tmp.path().join("escape.py"),
    )
    .unwrap();
    std::os::unix::fs::symlink(outside.path(), tmp.path().join("linked-dir")).unwrap();

    let bundle = Bundle::load(tmp.path()).unwrap();
    let ids: Vec<String> = bundle.concepts().iter().map(|c| c.id.to_string()).collect();
    // The link that stays inside is a concept; the one that leaves is not
    // read, and the linked directory is not walked.
    assert_eq!(ids, vec!["a", "inside", "real"]);

    // A path field that leads out of the root is external: never read.
    let a = ConceptId::parse("a").unwrap();
    assert_eq!(
        bundle.resolve_path(&a, "/escape.py"),
        PathResolution::External
    );
    assert_eq!(bundle.resolve_path_field(&a, "/escape.py"), None);
    assert_eq!(
        bundle.resolve_path(&a, "../../etc/passwd"),
        PathResolution::External
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
fn unquoted_okf_version_coerces_to_string() {
    let tmp = TempDir::new();
    tmp.write("a.md", "---\ntype: Note\n---\nbody\n");
    tmp.write("index.md", "---\nokf_version: 0.2\n---\n\n# Listing\n");
    let bundle = Bundle::load(tmp.path()).unwrap();
    assert_eq!(bundle.okf_version(), Some("0.2"));

    let tmp_int = TempDir::new();
    tmp_int.write("a.md", "---\ntype: Note\n---\nbody\n");
    tmp_int.write("index.md", "---\nokf_version: 1\n---\n\n# Listing\n");
    let bundle_int = Bundle::load(tmp_int.path()).unwrap();
    assert_eq!(bundle_int.okf_version(), Some("1"));
}

#[test]
fn load_single_file_as_bundle() {
    let tmp = TempDir::new();
    let file_path = tmp.write(
        "concept.md",
        "---\ntype: Note\ntitle: Single Concept\n---\n\nBody here.\n",
    );
    let bundle = Bundle::load_file(&file_path).unwrap();
    assert_eq!(bundle.len(), 1);
    assert_eq!(bundle.concepts()[0].id.to_string(), "concept");
    assert_eq!(
        bundle.concepts()[0].document.frontmatter.title().as_deref(),
        Some("Single Concept")
    );
}

#[test]
fn a_filename_that_is_not_a_valid_concept_id_segment_still_loads() {
    // Conformance is a question of frontmatter, not filenames, and the
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

    // Still conformant: the name is only worth a warning, since
    // conformance is a question of frontmatter.
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
fn a_file_that_is_not_utf8_is_reported_and_the_rest_still_loads() {
    // >PARALLEL_THRESHOLD files so the worker-thread path is taken. A
    // mid-list file is not UTF-8. That is a fault of the one file, like
    // frontmatter that does not parse: it is recorded against the file, and
    // the other concepts load, in path order.
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
    std::fs::write(tmp.path().join("f.md"), b"\xff\xfe not utf-8").unwrap();

    let bundle = Bundle::load(tmp.path()).unwrap();
    let ids: Vec<String> = bundle.concepts().iter().map(|c| c.id.to_string()).collect();
    let expected: Vec<String> = names
        .iter()
        .filter(|name| **name != "f")
        .map(|name| (*name).to_string())
        .collect();
    assert_eq!(ids, expected);

    let [(path, error)] = bundle.parse_errors() else {
        panic!("expected one parse error: {:?}", bundle.parse_errors());
    };
    assert!(path.ends_with("f.md"));
    assert!(matches!(error, DocumentError::NotUtf8));
    assert!(error.to_string().contains("not valid UTF-8"));
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

#[test]
fn paths_are_case_sensitive_on_every_file_system() {
    // On a case-insensitive file system `Tables/Orders.md` would open
    // `tables/orders.md`. Paths are case-sensitive, so it must not resolve,
    // or the same bundle would read differently on another machine.
    let tmp = TempDir::new();
    tmp.write("tables/orders.md", "---\ntype: Table\n---\nbody\n");
    tmp.write("attesters/check.py", "print(1)\n");
    tmp.write(
        "a.md",
        "---\ntype: Note\nsources:\n  - { resource: /Tables/Orders.md }\n---\n\
         See [orders](Tables/Orders.md) and the [attester](attesters/Check.py).\n",
    );
    let bundle = Bundle::load(tmp.path()).unwrap();
    let a = ConceptId::parse("a").unwrap();

    assert!(
        bundle
            .links_from(&a)
            .iter()
            .all(okf_core::ResolvedLink::is_broken)
    );
    assert_eq!(
        bundle.resolve_path(&a, "/Tables/Orders.md"),
        PathResolution::Missing
    );
    assert!(bundle.derived_from(&a).is_empty());
    assert!(matches!(
        bundle.resolve_path(&a, "/tables/orders.md"),
        PathResolution::Found(_)
    ));
}

#[cfg(unix)]
#[test]
fn a_root_index_that_links_out_of_the_bundle_is_not_its_metadata() {
    // The root `index.md` is a symbolic link to a file outside the bundle.
    // What that file says is not this bundle's metadata, and regenerating the
    // indexes must not write through the link.
    let outside = TempDir::new();
    let foreign = "---\nokf_version: \"0.3\"\ntitle: Somebody Else's Bundle\n---\n\n# Elsewhere\n";
    outside.write("index.md", foreign);

    let tmp = TempDir::new();
    tmp.write("a.md", "---\ntype: Note\ntitle: A\n---\nBody.\n");
    std::os::unix::fs::symlink(outside.path().join("index.md"), tmp.path().join("index.md"))
        .unwrap();

    let bundle = Bundle::load(tmp.path()).unwrap();
    assert_eq!(bundle.metadata().title, None);
    assert_eq!(bundle.okf_version(), None);

    okf_core::index::regenerate_indexes(tmp.path()).unwrap();
    assert_eq!(outside.read("index.md"), foreign);
}

#[test]
fn a_link_to_a_directory_is_not_a_link_to_the_concept_of_that_name() {
    // `[x](/metrics)` names the directory `metrics/`, which exists. The
    // concept `metrics.md` next to it is a different target: the `.md`-less
    // reading only applies when the target names nothing on disk.
    let tmp = TempDir::new();
    tmp.write("metrics.md", "---\ntype: Note\n---\nAbout metrics.\n");
    tmp.write("metrics/revenue.md", "---\ntype: Metric\n---\nRevenue.\n");
    tmp.write(
        "a.md",
        "---\ntype: Note\n---\nSee [the folder](/metrics) and [the note](/metrics.md) \
         and [short](/metrics/revenue).\n",
    );
    let bundle = Bundle::load(tmp.path()).unwrap();
    let a = ConceptId::parse("a").unwrap();
    let links = bundle.links_from(&a);
    assert_eq!(links.len(), 3);
    assert_eq!(links[0].resolution, LinkTarget::File);
    assert_eq!(links[1].resolution, LinkTarget::Concept);
    assert_eq!(links[1].target.to_string(), "metrics");
    // Nothing is named `metrics/revenue`, so the concept is what was meant.
    assert_eq!(links[2].resolution, LinkTarget::Concept);
    assert_eq!(links[2].target.to_string(), "metrics/revenue");
}
