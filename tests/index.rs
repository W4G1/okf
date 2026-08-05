//! Index generation tests, mirroring the reference `tests/test_index.py`.

mod common;

use common::TempDir;
use okf::index::{
    build_index_text, default_synthesize, regenerate_indexes, regenerate_indexes_with, IndexEntry,
};
use okf::links::extract_links;

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
    // §4.1: "If omitted, consumers MAY derive a title from the filename." The
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
            .resolve_all(&okf::ConceptId::parse("index").unwrap())
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

    assert!(tmp
        .read("index.md")
        .starts_with("---\nokf_version: \"0.2\"\n---\n"));
    let nested = tmp.read("tables/index.md");
    assert!(!nested.starts_with("---"), "{nested}");
    assert!(!nested.contains("okf_version"), "{nested}");
    assert!(!nested.contains("title: Wrong"), "{nested}");
}
