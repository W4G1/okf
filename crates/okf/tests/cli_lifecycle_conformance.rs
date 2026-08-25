//! Comprehensive end-to-end lifecycle conformance and lint tests.
//!
//! Asserts that ANY bundle or concept created or manipulated through `okf` CLI commands
//! (`init`, `new`, `index`, `fmt`, `mv`, `split`, `merge`, `rm`, `validate --fix`, `lint --fix`)
//! always passes `okf validate`, `okf lint`, and `okf fmt --check` without errors or warnings.

mod common;

use common::TempDir;
use std::process::Command;

fn okf() -> Command {
    Command::new(env!("CARGO_BIN_EXE_okf"))
}

fn assert_bundle_is_clean(bundle_dir: &std::path::Path) {
    let bundle_str = bundle_dir.to_str().unwrap();

    // 1. okf fmt --check
    let fmt_check = okf().args(["fmt", bundle_str, "--check"]).output().unwrap();
    assert_eq!(
        fmt_check.status.code(),
        Some(0),
        "fmt --check failed on {bundle_str}:\nSTDOUT: {}\nSTDERR: {}",
        String::from_utf8_lossy(&fmt_check.stdout),
        String::from_utf8_lossy(&fmt_check.stderr)
    );

    // 2. okf validate --json
    let val_out = okf()
        .args(["validate", bundle_str, "--json"])
        .output()
        .unwrap();
    assert_eq!(
        val_out.status.code(),
        Some(0),
        "validate failed on {bundle_str}:\nSTDOUT: {}\nSTDERR: {}",
        String::from_utf8_lossy(&val_out.stdout),
        String::from_utf8_lossy(&val_out.stderr)
    );
    let val_json: serde_json::Value = serde_json::from_slice(&val_out.stdout).unwrap();
    assert_eq!(
        val_json["conformant"].as_bool(),
        Some(true),
        "Bundle not conformant: {val_json:#?}"
    );
    assert_eq!(
        val_json["error_count"].as_i64(),
        Some(0),
        "Validation errors present: {val_json:#?}"
    );
    assert_eq!(
        val_json["warning_count"].as_i64(),
        Some(0),
        "Validation warnings present: {val_json:#?}"
    );

    // 3. okf lint --json
    let lint_out = okf().args(["lint", bundle_str, "--json"]).output().unwrap();
    assert_eq!(
        lint_out.status.code(),
        Some(0),
        "lint failed on {bundle_str}:\nSTDOUT: {}\nSTDERR: {}",
        String::from_utf8_lossy(&lint_out.stdout),
        String::from_utf8_lossy(&lint_out.stderr)
    );
    let lint_json: serde_json::Value = serde_json::from_slice(&lint_out.stdout).unwrap();
    assert_eq!(
        lint_json["warning_count"].as_i64(),
        Some(0),
        "Lint warnings present: {lint_json:#?}"
    );
}

#[test]
fn test_init_default_and_bare_bundles_pass_all_checks() {
    let tmp = TempDir::new();

    // Default init with overview sample
    let default_bundle = tmp.path().join("default_bundle");
    let init_res = okf()
        .args(["init", default_bundle.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(init_res.status.success());
    assert_bundle_is_clean(&default_bundle);

    // Bare init
    let bare_bundle = tmp.path().join("bare_bundle");
    let bare_res = okf()
        .args(["init", bare_bundle.to_str().unwrap(), "--bare"])
        .output()
        .unwrap();
    assert!(bare_res.status.success());
    assert_bundle_is_clean(&bare_bundle);
}

#[test]
fn test_new_and_multi_directory_indexing_passes_all_checks() {
    let tmp = TempDir::new();
    let bundle_path = tmp.path().join("multi_dir_bundle");

    // Initialize bundle
    let init_res = okf()
        .args(["init", bundle_path.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(init_res.status.success());

    // Create concepts across 15 distinct subdirectories
    for i in 1..=15 {
        let rel_path = format!("category_{i}/concept_{i}");
        let title = format!("Concept {i}");
        let desc = format!("Description for concept {i}");
        let new_res = okf()
            .args([
                "new",
                bundle_path.to_str().unwrap(),
                &rel_path,
                "--type",
                "Metric",
                "--title",
                &title,
                "--description",
                &desc,
                "--status",
                "stable",
            ])
            .output()
            .unwrap();
        assert!(
            new_res.status.success(),
            "Failed creating {rel_path}: {}",
            String::from_utf8_lossy(&new_res.stderr)
        );
    }

    // Regenerate indexes across all subdirectories
    let idx_res = okf()
        .args(["index", bundle_path.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(idx_res.status.success());

    // Assert entire bundle is completely clean (fmt check, validate, lint)
    assert_bundle_is_clean(&bundle_path);

    // Run fmt -w and index again to ensure idempotent stability
    let fmt_w = okf()
        .args(["fmt", bundle_path.to_str().unwrap(), "-w"])
        .output()
        .unwrap();
    assert!(fmt_w.status.success());

    let idx_res2 = okf()
        .args(["index", bundle_path.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(idx_res2.status.success());

    assert_bundle_is_clean(&bundle_path);
}

#[test]
fn test_refactor_pipeline_lifecycle_passes_all_checks() {
    let tmp = TempDir::new();
    let bundle_path = tmp.path().join("refactor_bundle");

    // Init
    okf()
        .args(["init", bundle_path.to_str().unwrap()])
        .status()
        .unwrap();

    // Create concepts
    okf()
        .args([
            "new",
            bundle_path.to_str().unwrap(),
            "metrics/mrr",
            "--title",
            "MRR",
            "--description",
            "Monthly recurring revenue",
            "--status",
            "stable",
        ])
        .status()
        .unwrap();

    okf()
        .args([
            "new",
            bundle_path.to_str().unwrap(),
            "metrics/arr",
            "--title",
            "ARR",
            "--description",
            "Annual recurring revenue",
            "--status",
            "stable",
        ])
        .status()
        .unwrap();

    // Add a section to MRR for splitting
    let mrr_path = bundle_path.join("metrics/mrr.md");
    let mrr_content = "\
---
type: Concept
title: MRR
description: Monthly recurring revenue.
status: stable
generated:
  by: human:alice
  at: '2026-01-01T00:00:00Z'
---

# MRR

Monthly recurring revenue details.
See also [ARR](arr.md).

## Growth Rate
MRR growth rate formulas and explanations.
";
    std::fs::write(&mrr_path, mrr_content).unwrap();

    // 1. Split section
    let split_res = okf()
        .args([
            "split",
            "metrics/mrr",
            "metrics/growth_rate",
            "--bundle",
            bundle_path.to_str().unwrap(),
            "--section",
            "Growth Rate",
            "--title",
            "Growth Rate",
        ])
        .output()
        .unwrap();
    assert!(
        split_res.status.success(),
        "Split failed: {}",
        String::from_utf8_lossy(&split_res.stderr)
    );

    // 2. Move concept
    let mv_res = okf()
        .args([
            "mv",
            "metrics/growth_rate",
            "analytics/growth_rate",
            "--bundle",
            bundle_path.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        mv_res.status.success(),
        "Mv failed: {}",
        String::from_utf8_lossy(&mv_res.stderr)
    );

    // 3. Merge ARR into MRR
    let merge_res = okf()
        .args([
            "merge",
            "metrics/arr",
            "metrics/mrr",
            "--bundle",
            bundle_path.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        merge_res.status.success(),
        "Merge failed: {}",
        String::from_utf8_lossy(&merge_res.stderr)
    );

    // 4. Remove a concept with --unlink
    let rm_res = okf()
        .args([
            "rm",
            "overview",
            "--bundle",
            bundle_path.to_str().unwrap(),
            "--unlink",
            "--force",
        ])
        .output()
        .unwrap();
    assert!(
        rm_res.status.success(),
        "Rm failed: {}",
        String::from_utf8_lossy(&rm_res.stderr)
    );

    // Re-index bundle
    okf()
        .args(["index", bundle_path.to_str().unwrap()])
        .status()
        .unwrap();

    // Assert bundle is clean across all verifications
    assert_bundle_is_clean(&bundle_path);
}

#[test]
fn test_validate_and_lint_fix_resolves_all_issues_cleanly() {
    let tmp = TempDir::new();
    let bundle_path = tmp.path().join("fixable_bundle");

    // Initialize
    okf()
        .args(["init", bundle_path.to_str().unwrap(), "--bare"])
        .status()
        .unwrap();

    // Create concept with date-only, missing title, legacy timestamp, etc.
    let messy_path = bundle_path.join("concepts/legacy.md");
    std::fs::create_dir_all(messy_path.parent().unwrap()).unwrap();
    let messy_content = "\
---
type: Concept
description: Legacy concept description.
timestamp: '2026-06-30'
stale_after: '2026-12-31'
usage_window:
  from: '2026-01-01'
  to: '2026-06-01'
sources:
  - id: '1'
    resource: https://example.com/data
    last_modified: '2026-05-15'
---

Legacy concept content with footnote [^1].

[^1]: https://example.com/data
";
    std::fs::write(&messy_path, messy_content).unwrap();

    // Run validate --fix
    let val_fix = okf()
        .args(["validate", bundle_path.to_str().unwrap(), "--fix"])
        .output()
        .unwrap();
    assert!(val_fix.status.success());

    // Run lint --fix
    let lint_fix = okf()
        .args(["lint", bundle_path.to_str().unwrap(), "--fix"])
        .output()
        .unwrap();
    assert!(lint_fix.status.success());

    // Re-index bundle
    okf()
        .args(["index", bundle_path.to_str().unwrap()])
        .status()
        .unwrap();

    // Assert bundle is completely clean
    assert_bundle_is_clean(&bundle_path);
}
