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

#[test]
fn cli_links_short_flags_work() {
    let tmp = TempDir::new();
    let bundle_path = tmp.path().join("links_short");

    tmp.write(
        "links_short/index.md",
        "---\nokf_version: \"0.2\"\n---\n\n# Index\n\n* [A](a.md)\n",
    );
    tmp.write(
        "links_short/a.md",
        "---\ntype: Concept\ntitle: A\n---\n\n# A\n\n[Broken](missing.md) and [Ext](https://example.com)\n",
    );

    // 1. okf links -b
    let broken_out = okf()
        .args(["links", bundle_path.to_str().unwrap(), "-b"])
        .output()
        .unwrap();
    assert_eq!(broken_out.status.code(), Some(0));
    let broken_text = String::from_utf8_lossy(&broken_out.stdout);
    assert!(broken_text.contains("a -> missing.md"));

    // 2. okf links -b -c (should exit 65)
    let check_status = okf()
        .args(["links", bundle_path.to_str().unwrap(), "-b", "-c"])
        .status()
        .unwrap();
    assert_eq!(code(check_status), 65);

    // 3. okf links -a (should include external link)
    let all_out = okf()
        .args(["links", bundle_path.to_str().unwrap(), "-a"])
        .output()
        .unwrap();
    assert_eq!(all_out.status.code(), Some(0));
    let all_text = String::from_utf8_lossy(&all_out.stdout);
    assert!(all_text.contains("https://example.com"));
}

#[test]
fn cli_new_with_bundle_flag_and_validation() {
    let tmp = TempDir::new();
    let bundle_path = tmp.path().join("flag_bundle");

    okf()
        .args(["init", bundle_path.to_str().unwrap(), "--bare"])
        .status()
        .unwrap();

    let status = okf()
        .args([
            "new",
            "metrics/active_users",
            "--bundle",
            bundle_path.to_str().unwrap(),
            "--title",
            "Active Users",
        ])
        .status()
        .unwrap();
    assert_eq!(code(status), 0);
    assert!(bundle_path.join("metrics/active_users.md").is_file());

    let invalid_status = okf()
        .args([
            "new",
            "metrics/../evil",
            "--bundle",
            bundle_path.to_str().unwrap(),
        ])
        .status()
        .unwrap();
    assert_ne!(code(invalid_status), 0);
}

#[test]
fn cli_fmt_and_parse_accept_concept_id_without_md() {
    let tmp = TempDir::new();
    let bundle_path = tmp.path().join("fmt_parse_bundle");

    okf()
        .args(["init", bundle_path.to_str().unwrap()])
        .status()
        .unwrap();

    let overview_stem = bundle_path.join("overview");

    let parse_output = okf()
        .args(["parse", overview_stem.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    assert_eq!(parse_output.status.code(), Some(0));
    let parse_json = String::from_utf8_lossy(&parse_output.stdout);
    assert!(parse_json.contains("\"okf_version\": \"0.2\""));
    assert!(parse_json.contains("\"conformant\": true"));

    let fmt_output = okf()
        .args(["fmt", overview_stem.to_str().unwrap(), "--check", "--json"])
        .output()
        .unwrap();
    assert_eq!(fmt_output.status.code(), Some(0));
    let fmt_json = String::from_utf8_lossy(&fmt_output.stdout);
    assert!(fmt_json.contains("\"okf_version\": \"0.2\""));
    assert!(fmt_json.contains("\"clean\": true"));
}

#[test]
fn cli_validate_and_lint_default_to_current_directory() {
    let tmp = TempDir::new();
    tmp.write(
        "index.md",
        "---\nokf_version: \"0.2\"\n---\n\n# Root Bundle\n\n* [Overview](overview.md)\n",
    );
    tmp.write(
        "overview.md",
        "---\ntype: Concept\ntitle: Overview\ndescription: d\ngenerated: { by: human:me, at: 2026-01-01T00:00:00Z }\n---\n\n# Overview\n\nOverview content.\n",
    );

    let val_output = Command::new(env!("CARGO_BIN_EXE_okf"))
        .arg("validate")
        .current_dir(tmp.path())
        .output()
        .unwrap();
    assert_eq!(val_output.status.code(), Some(0));

    let lint_output = Command::new(env!("CARGO_BIN_EXE_okf"))
        .arg("lint")
        .current_dir(tmp.path())
        .output()
        .unwrap();
    assert_eq!(lint_output.status.code(), Some(0));
}
