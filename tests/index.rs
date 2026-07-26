//! Index generation tests, mirroring the reference `tests/test_index.py`.

mod common;

use common::TempDir;
use okf::index::{default_synthesize, regenerate_indexes, regenerate_indexes_with};

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
    write_doc(&tmp, "datasets/ga4.md", "BigQuery Dataset", "GA4 Dataset", "GA4 obfuscated ecommerce sample.");
    write_doc(&tmp, "tables/events_.md", "BigQuery Table", "events_*", "Daily-sharded GA4 event tables.");
    write_doc(&tmp, "tables/users.md", "BigQuery Table", "users", "Per-user dimension.");

    // Deterministic synthesizer so we can assert on the root index text.
    let synth = |_rel: &str, children: &[(String, String)]| format!("stub: {} items", children.len());
    let written = regenerate_indexes_with(tmp.path(), &synth).unwrap();
    let dirs = written_dirs(&written);
    let root = tmp.path().file_name().unwrap().to_str().unwrap().to_string();
    assert!(dirs.contains(&"datasets".to_string()), "{dirs:?}");
    assert!(dirs.contains(&"tables".to_string()), "{dirs:?}");
    assert!(dirs.contains(&root), "the bundle root gets an index too: {dirs:?}");

    let tables_index = tmp.read("tables/index.md");
    assert!(tables_index.starts_with("# BigQuery Table"), "{tables_index}");
    assert!(tables_index.contains("[events_*](events_.md)"), "{tables_index}");
    assert!(tables_index.contains("[users](users.md)"), "{tables_index}");
    assert!(tables_index.contains("Daily-sharded GA4 event tables."));

    let root_index = tmp.read("index.md");
    assert!(root_index.contains("# Subdirectories"), "{root_index}");
    assert!(root_index.contains("(datasets/index.md) - GA4 obfuscated ecommerce sample."), "{root_index}");
    assert!(root_index.contains("(tables/index.md) - stub: 2 items"), "{root_index}");
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
    write_doc(&tmp, "datasets/only.md", "BigQuery Dataset", "Only Dataset", "The only dataset in this bundle.");

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
    assert_eq!(calls.get(), 0, "single child with a description should be reused, not synthesized");
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
    assert!(tables_index.contains("[orders](orders.md)"), "{tables_index}");
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
