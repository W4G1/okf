//! Integration tests for okf CLI subcommands: init, new, fmt, links.

mod common;

use common::TempDir;
use std::process::{Command, ExitStatus};

fn okf() -> Command {
    Command::new(env!("CARGO_BIN_EXE_okf"))
}

fn code(status: ExitStatus) -> i32 {
    status.code().unwrap_or(0)
}

#[test]
fn cli_init_creates_valid_bundle() {
    let tmp = TempDir::new();
    let bundle_path = tmp.path().join("demo_bundle");

    let status = okf()
        .args(["init", bundle_path.to_str().unwrap()])
        .status()
        .unwrap();
    assert_eq!(code(status), 0);

    // Validate the created bundle with okf validate
    let val_status = okf()
        .args(["validate", bundle_path.to_str().unwrap()])
        .status()
        .unwrap();
    assert_eq!(code(val_status), 0);

    // Lint the created bundle with okf lint
    let lint_status = okf()
        .args(["lint", bundle_path.to_str().unwrap()])
        .status()
        .unwrap();
    assert_eq!(code(lint_status), 0);
}

#[test]
fn cli_new_creates_concepts() {
    let tmp = TempDir::new();
    let bundle_path = tmp.path().join("my_bundle");

    // Init bundle first
    okf()
        .args(["init", bundle_path.to_str().unwrap(), "--bare"])
        .status()
        .unwrap();

    // Create a new metric concept
    let status = okf()
        .args([
            "new",
            bundle_path.to_str().unwrap(),
            "metrics/mrr",
            "--type",
            "Metric",
            "--title",
            "Monthly Recurring Revenue",
            "--description",
            "Normalized MRR figure.",
        ])
        .status()
        .unwrap();
    assert_eq!(code(status), 0);

    let mrr_file = bundle_path.join("metrics/mrr.md");
    assert!(mrr_file.is_file());

    // Create an attested computation
    let comp_status = okf()
        .args([
            "new",
            bundle_path.to_str().unwrap(),
            "computations/mrr_calc",
            "--attested",
            "--title",
            "MRR Calculator",
        ])
        .status()
        .unwrap();
    assert_eq!(code(comp_status), 0);

    let comp_file = bundle_path.join("computations/mrr_calc.md");
    assert!(comp_file.is_file());
}

#[test]
fn cli_fmt_bundle_directory_recursively() {
    let tmp = TempDir::new();
    let bundle_path = tmp.path().join("fmt_bundle");

    okf()
        .args(["init", bundle_path.to_str().unwrap()])
        .status()
        .unwrap();

    // Create unformatted file
    let messy_file = bundle_path.join("notes.md");
    std::fs::write(
        &messy_file,
        "---\ntype: Note\ntitle:   My Note  \n---\n\n# Note Body\n",
    )
    .unwrap();

    // Run okf fmt -w on the directory
    let status = okf()
        .args(["fmt", bundle_path.to_str().unwrap(), "-w"])
        .status()
        .unwrap();
    assert_eq!(code(status), 0);

    let content = std::fs::read_to_string(&messy_file).unwrap();
    assert!(content.starts_with("---\n"));
    assert!(content.contains("type: Note\n"));
}

#[test]
fn cli_links_inspects_internal_and_broken_links() {
    let tmp = TempDir::new();
    let bundle_path = tmp.path().join("links_bundle");

    tmp.write(
        "links_bundle/index.md",
        "---\nokf_version: \"0.2\"\n---\n\n# Index\n\n* [A](a.md)\n* [B](b.md)\n",
    );
    tmp.write(
        "links_bundle/a.md",
        "---\ntype: Metric\ntitle: A\ndescription: d\n\
         generated: { by: ref/x, at: 2026-06-20T22:53:05Z }\n\
         verified: { by: human:a, at: 2026-06-25T09:00:00Z }\n\
         ---\n\n# Definition\n\nLink to [B](b.md) and [Missing](missing.md) and [External](https://example.com).\n",
    );
    tmp.write(
        "links_bundle/b.md",
        "---\ntype: Metric\ntitle: B\ndescription: d\n\
         generated: { by: ref/x, at: 2026-06-20T22:53:05Z }\n\
         verified: { by: human:a, at: 2026-06-25T09:00:00Z }\n\
         ---\n\n# Definition\n\nLink to [A](a.md).\n",
    );

    // okf links default
    let output = okf()
        .args(["links", bundle_path.to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("-> b [ok]"));
    assert!(stdout.contains("-x missing [broken: missing.md]"));

    // okf links --broken
    let broken_output = okf()
        .args(["links", bundle_path.to_str().unwrap(), "--broken"])
        .output()
        .unwrap();
    assert_eq!(broken_output.status.code(), Some(0));
    let stdout_broken = String::from_utf8_lossy(&broken_output.stdout);
    assert!(stdout_broken.contains("a -> missing.md"));

    // okf links --broken --check (should return EX_DATAERR (65))
    let check_status = okf()
        .args([
            "links",
            bundle_path.to_str().unwrap(),
            "--broken",
            "--check",
        ])
        .status()
        .unwrap();
    assert_eq!(code(check_status), 65);

    // okf links --format json
    let json_output = okf()
        .args(["links", bundle_path.to_str().unwrap(), "--format", "json"])
        .output()
        .unwrap();
    assert_eq!(json_output.status.code(), Some(0));
    let stdout_json = String::from_utf8_lossy(&json_output.stdout);
    assert!(stdout_json.contains("\"broken_count\": 1"));
    assert!(stdout_json.contains("\"exists\": false"));
}
