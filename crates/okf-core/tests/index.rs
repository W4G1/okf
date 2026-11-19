//! Index generation tests, mirroring the reference `tests/test_index.py`.

mod common;

use common::TempDir;
use okf_core::index::{
    IndexEntry, build_index_text, default_synthesize, regenerate_indexes, regenerate_indexes_with,
};
use okf_core::links::extract_links;

fn write_doc(tmp: &TempDir, rel: &str, type_: &str, title: &str, description: &str) {
    let contents = format!(
        "---\ntype: {type_}\ntitle: {title}\ndescription: {description}\n\
         generated: {{ by: reference_agent/stub, at: '2026-05-27T00:00:00+00:00' }}\n\
         ---\n\n# {title}\n\n{description}\n"
    );
    tmp.write(rel, &contents);
}

/// The directory names an index was written into.
fn written_dirs(written: &[std::path::PathBuf]) -> Vec<String> {
    let mut names: Vec<String> = written
        .iter()
        .filter_map(|p| p.parent()?.file_name()?.to_str().map(String::from))
        .collect();
    names.sort();
    names
}

#[test]
fn regenerate_groups_by_type_and_links_relative() {
    let tmp = TempDir::new();
    write_doc(
        &tmp,
        "datasets/ga4.md",
        "BigQuery Dataset",
        "GA4 Dataset",
        "GA4 obfuscated ecommerce sample.",
    );
    write_doc(
        &tmp,
        "tables/events_.md",
        "BigQuery Table",
        "events_*",
        "Daily-sharded GA4 event tables.",
    );
    write_doc(
        &tmp,
        "tables/users.md",
        "BigQuery Table",
        "users",
        "Per-user dimension.",
    );

    // Deterministic synthesizer so we can assert on the root index text.
    let synth =
        |_rel: &str, children: &[(String, String)]| format!("stub: {} items", children.len());
    let written = regenerate_indexes_with(tmp.path(), &synth).unwrap();
    let dirs = written_dirs(&written);
    let root = tmp
        .path()
        .file_name()
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert!(dirs.contains(&"datasets".to_string()), "{dirs:?}");
    assert!(dirs.contains(&"tables".to_string()), "{dirs:?}");
    assert!(
        dirs.contains(&root),
        "the bundle root gets an index too: {dirs:?}"
    );

    let tables_index = tmp.read("tables/index.md");
    assert!(
        tables_index.starts_with("# BigQuery Table"),
        "{tables_index}"
    );
    assert!(
        tables_index.contains("[events_*](events_.md)"),
        "{tables_index}"
    );
    assert!(tables_index.contains("[users](users.md)"), "{tables_index}");
    assert!(tables_index.contains("Daily-sharded GA4 event tables."));

    let root_index = tmp.read("index.md");
    assert!(root_index.contains("# Subdirectories"), "{root_index}");
    assert!(
        root_index.contains("(datasets/index.md) - GA4 obfuscated ecommerce sample."),
        "{root_index}"
    );
    assert!(
        root_index.contains("(tables/index.md) - stub: 2 items"),
        "{root_index}"
    );
}

#[test]
fn regenerate_skips_empty_directories() {
    let tmp = TempDir::new();
    tmp.mkdir("empty_dir");
    let written = regenerate_indexes(tmp.path()).unwrap();
    assert!(written.is_empty());
    assert!(!tmp.path().join("empty_dir/index.md").exists());
}

#[test]
fn regenerate_single_child_reuses_description() {
    let tmp = TempDir::new();
    write_doc(
        &tmp,
        "datasets/only.md",
        "BigQuery Dataset",
        "Only Dataset",
        "The only dataset in this bundle.",
    );

    let calls = std::cell::Cell::new(0u32);
    let counting = |_rel: &str, children: &[(String, String)]| {
        calls.set(calls.get() + 1);
        format!("stub: {} items", children.len())
    };
    regenerate_indexes_with(tmp.path(), &counting).unwrap();

    let root_index = tmp.read("index.md");
    assert!(
        root_index.contains("(datasets/index.md) - The only dataset in this bundle."),
        "{root_index}"
    );
    assert_eq!(
        calls.get(),
        0,
        "single child with a description should be reused, not synthesized"
    );
}

#[test]
fn an_empty_title_falls_back_to_the_filename() {
    // Spec: "If omitted, consumers MAY derive a title from the filename." The
    // reference's `fm.get("title") or child.stem` treats an empty title as
    // omitted, so an empty string must not become the link text.
    let tmp = TempDir::new();
    tmp.write(
        "tables/orders.md",
        "---\ntype: BigQuery Table\ntitle: ''\ndescription: Order lines.\n---\n\nProse.\n",
    );
    regenerate_indexes(tmp.path()).unwrap();

    let tables_index = tmp.read("tables/index.md");
    assert!(
        tables_index.contains("[orders](orders.md)"),
        "{tables_index}"
    );
}

#[test]
fn the_default_synthesizer_matches_the_reference_fallback() {
    // Same wording as the reference `synthesize_description`'s `_fallback`, so
    // an index generated without a model reads the same in both.
    let children = [
        ("Orders".to_string(), "Order lines.".to_string()),
        ("Users".to_string(), String::new()),
    ];
    assert_eq!(
        default_synthesize("tables", &children),
        "Contains 2 entries: Orders, Users."
    );

    let untitled = [(String::new(), "no title".to_string())];
    assert_eq!(
        default_synthesize("tables", &untitled),
        "Contains 1 entries: no titled entries."
    );

    assert_eq!(default_synthesize("tables", &[]), "");
}

#[test]
fn generated_markdown_escapes_text_and_encodes_destinations() {
    let entries = [IndexEntry {
        type_: "Metric".to_string(),
        title: "Revenue ](https://evil.example)".to_string(),
        link: "tables/my notes (v2) #1%.md".to_string(),
        description: "Description\n* [injected](https://evil.example)".to_string(),
    }];
    let text = build_index_text(&entries);

    assert!(text.contains("tables/my%20notes%20%28v2%29%20%231%25.md"));
    assert!(text.contains(r"\](https://evil.example)"));
    assert!(text.contains(r"\[injected\]"));
    assert_eq!(extract_links(&text).len(), 1);
    assert_eq!(
        extract_links(&text)[0].target,
        "tables/my%20notes%20%28v2%29%20%231%25.md"
    );
    assert_eq!(
        extract_links(&text)[0]
            .resolve_all(&okf_core::ConceptId::parse("index").unwrap())
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>(),
        vec![
            "tables/my%20notes%20%28v2%29%20%231%25".to_string(),
            "tables/my notes (v2) #1%".to_string(),
        ]
    );
}

#[test]
fn regenerated_indexes_encode_filesystem_names_once() {
    let tmp = TempDir::new();
    write_doc(
        &tmp,
        "tables/my notes (v2) #1%.md",
        "Metric",
        "Weird filename",
        "A description.",
    );

    regenerate_indexes(tmp.path()).unwrap();

    let index = tmp.read("tables/index.md");
    assert!(
        index.contains("[Weird filename](my%20notes%20%28v2%29%20%231%25.md)"),
        "{index}"
    );
    assert!(!index.contains("%2520"), "{index}");
}

#[test]
fn nested_indexes_do_not_keep_root_version_frontmatter() {
    let tmp = TempDir::new();
    tmp.write("index.md", "---\nokf_version: '0.2'\n---\n\n# Bundle\n");
    tmp.write(
        "tables/index.md",
        "---\nokf_version: '0.1'\ntitle: Wrong\n---\n\n# Old\n",
    );
    write_doc(&tmp, "tables/orders.md", "Metric", "Orders", "Order count.");

    regenerate_indexes(tmp.path()).unwrap();

    // The root block is kept exactly as written, quoting included.
    assert!(
        tmp.read("index.md")
            .starts_with("---\nokf_version: '0.2'\n---\n\n# Subdirectories\n")
    );
    let nested = tmp.read("tables/index.md");
    assert!(!nested.starts_with("---"), "{nested}");
    assert!(!nested.contains("okf_version"), "{nested}");
    assert!(!nested.contains("title: Wrong"), "{nested}");
}

#[test]
fn regenerate_keeps_the_bundle_metadata_verbatim() {
    // A producer that regenerates index.md must keep the existing frontmatter
    // of the bundle-root index: the metadata, extension keys, and comments.
    let frontmatter = "---\n\
        okf_version: \"0.3\"\n\
        title: Acme retail analytics\n\
        # Agents read this to choose a bundle.\n\
        description: Tables and metrics for Acme retail. Use it for sales questions.\n\
        license: CC-BY-4.0\n\
        x-acme-owner: { team: data, oncall: \"#data-oncall\" }\n\
        ---\n";
    let tmp = TempDir::new();
    tmp.write(
        "index.md",
        &format!("{frontmatter}\n# Old listing\n\n* [Gone](gone.md)\n"),
    );
    write_doc(
        &tmp,
        "tables/users.md",
        "BigQuery Table",
        "users",
        "Per-user dimension.",
    );

    regenerate_indexes(tmp.path()).unwrap();

    let root = tmp.read("index.md");
    assert!(
        root.starts_with(&format!("{frontmatter}\n# Subdirectories")),
        "{root}"
    );
    assert!(!root.contains("Old listing"));

    // Regenerating again changes nothing.
    regenerate_indexes(tmp.path()).unwrap();
    assert_eq!(tmp.read("index.md"), root);

    let bundle = okf_core::Bundle::load(tmp.path()).unwrap();
    let metadata = bundle.metadata();
    assert_eq!(metadata.okf_version.as_deref(), Some("0.3"));
    assert_eq!(metadata.title.as_deref(), Some("Acme retail analytics"));
    assert_eq!(metadata.license.as_deref(), Some("CC-BY-4.0"));
    assert!(metadata.frontmatter.get("x-acme-owner").is_some());
}

#[test]
fn a_new_root_index_declares_the_version() {
    let tmp = TempDir::new();
    write_doc(
        &tmp,
        "tables/users.md",
        "BigQuery Table",
        "users",
        "Per-user dimension.",
    );

    regenerate_indexes(tmp.path()).unwrap();

    assert!(
        tmp.read("index.md")
            .starts_with("---\nokf_version: \"0.3\"\n---\n\n# Subdirectories")
    );
    assert!(!tmp.read("tables/index.md").starts_with("---"));
}

#[test]
fn the_version_is_not_declared_while_a_path_depends_on_the_root_fallback() {
    // `references/run.md` on a concept in `computations/` only resolves when
    // read from the bundle root, which is how v0.2 wrote it. Declaring 0.3
    // would stop it from resolving, so the index is written without it.
    let tmp = TempDir::new();
    tmp.write(
        "computations/revenue.md",
        "---\ntype: Attested Computation\nruntime: bigquery\nexecutor:\n  resource: references/run.md\n  receipt: [job_id]\n---\n\n# Computation\n\n```sql\nSELECT 1\n```\n",
    );
    write_doc(
        &tmp,
        "references/run.md",
        "Skill",
        "Run",
        "Run instructions.",
    );

    regenerate_indexes(tmp.path()).unwrap();
    assert!(tmp.read("index.md").starts_with("# Subdirectories"));
}

#[test]
fn files_that_are_not_concepts_are_not_listed() {
    let tmp = TempDir::new();
    write_doc(
        &tmp,
        "tables/users.md",
        "BigQuery Table",
        "users",
        "Per-user dimension.",
    );
    tmp.write("README.md", "# Read me\n");
    tmp.write("tables/NOTES.md", "---\ntitle: No type\n---\nScratch.\n");
    tmp.write("docs/README.md", "# Only prose here\n");
    tmp.write(".github/x.md", "---\ntype: Hidden\n---\n");
    tmp.write(
        "attesters/index.md",
        "# Code\n\n* [sql](sql.py) - Attester.\n",
    );
    tmp.write("attesters/sql.py", "print(1)\n");

    regenerate_indexes(tmp.path()).unwrap();

    let root = tmp.read("index.md");
    assert!(!root.contains("README"), "{root}");
    assert!(root.contains("[tables](tables/index.md)"), "{root}");
    // A directory with nothing to list is linked only if it has an index.
    assert!(root.contains("[attesters](attesters/index.md)"), "{root}");
    assert!(!root.contains("docs"), "{root}");
    assert!(!root.contains(".github"), "{root}");
    assert!(!tmp.path().join("docs/index.md").exists());
    // The hand-written index of an asset directory is left alone.
    assert_eq!(
        tmp.read("attesters/index.md"),
        "# Code\n\n* [sql](sql.py) - Attester.\n"
    );

    let tables = tmp.read("tables/index.md");
    assert!(tables.contains("[users](users.md)"));
    assert!(!tables.contains("NOTES"), "{tables}");
}

#[test]
fn regenerate_skips_reserved_log_file() {
    let tmp = TempDir::new();
    tmp.write(
        "log.md",
        "# Update Log\n\n## 2026-08-18\n* **Creation**: Initialized.\n",
    );
    write_doc(
        &tmp,
        "overview.md",
        "Overview",
        "Overview",
        "Main overview.",
    );

    regenerate_indexes(tmp.path()).unwrap();

    let root_index = tmp.read("index.md");
    assert!(
        root_index.contains("[Overview](overview.md)"),
        "{root_index}"
    );
    assert!(!root_index.contains("log.md"), "{root_index}");
    assert!(!root_index.contains("# Other"), "{root_index}");
}
