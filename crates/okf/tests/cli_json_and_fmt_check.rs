//! Integration tests for okf CLI universal --json flags and fmt --check.

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
fn cli_fmt_check_detects_unformatted_files() {
    let tmp = TempDir::new();
    let bundle_path = tmp.path().join("check_bundle");

    // Init bundle
    okf()
        .args(["init", bundle_path.to_str().unwrap()])
        .status()
        .unwrap();

    // Verify initial bundle is cleanly formatted
    let clean_check = okf()
        .args(["fmt", bundle_path.to_str().unwrap(), "--check"])
        .status()
        .unwrap();
    assert_eq!(code(clean_check), 0);

    // Create unformatted file
    let messy_file = bundle_path.join("unformatted.md");
    std::fs::write(
        &messy_file,
        "---\ntype: Note\ntitle:   Messy Note  \n---\n\n# Note Body\n",
    )
    .unwrap();

    // Run fmt --check in text mode (should exit with EX_DATAERR 65)
    let output = okf()
        .args(["fmt", bundle_path.to_str().unwrap(), "--check"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(65));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("needs formatting:"));
    assert!(stdout.contains("unformatted.md"));
    assert!(stdout.contains("would be reformatted"));

    // Run fmt --check --json mode
    let json_output = okf()
        .args(["fmt", bundle_path.to_str().unwrap(), "--check", "--json"])
        .output()
        .unwrap();
    assert_eq!(json_output.status.code(), Some(65));
    let stdout_json = String::from_utf8_lossy(&json_output.stdout);
    assert!(stdout_json.contains("\"clean\": false"));
    assert!(stdout_json.contains("\"unformatted_count\": 1"));
    assert!(stdout_json.contains("unformatted.md"));

    // Single-file check
    let single_output = okf()
        .args(["fmt", messy_file.to_str().unwrap(), "-c"])
        .output()
        .unwrap();
    assert_eq!(single_output.status.code(), Some(65));

    // Fix formatting with -w
    let fix_status = okf()
        .args(["fmt", bundle_path.to_str().unwrap(), "-w"])
        .status()
        .unwrap();
    assert_eq!(code(fix_status), 0);

    // Now fmt --check should pass
    let after_fix_check = okf()
        .args(["fmt", bundle_path.to_str().unwrap(), "--check", "-j"])
        .output()
        .unwrap();
    assert_eq!(after_fix_check.status.code(), Some(0));
    let after_json = String::from_utf8_lossy(&after_fix_check.stdout);
    assert!(after_json.contains("\"clean\": true"));
    assert!(after_json.contains("\"unformatted_count\": 0"));
}

#[test]
fn cli_universal_json_flags() {
    let tmp = TempDir::new();
    let bundle_path = tmp.path().join("json_bundle");

    // 1. okf init --json
    let init_out = okf()
        .args([
            "init",
            bundle_path.to_str().unwrap(),
            "--title",
            "JSON Test Bundle",
            "--json",
        ])
        .output()
        .unwrap();
    assert_eq!(init_out.status.code(), Some(0));
    let init_json = String::from_utf8_lossy(&init_out.stdout);
    assert!(init_json.contains("\"status\": \"ok\""));
    assert!(init_json.contains("\"title\": \"JSON Test Bundle\""));
    assert!(init_json.contains("index.md"));

    // 2. okf new --json
    let new_out = okf()
        .args([
            "new",
            bundle_path.to_str().unwrap(),
            "computations/revenue",
            "--type",
            "Attested Computation",
            "--title",
            "Revenue Calculator",
            "--attested",
            "--json",
        ])
        .output()
        .unwrap();
    assert_eq!(new_out.status.code(), Some(0));
    let new_json = String::from_utf8_lossy(&new_out.stdout);
    assert!(new_json.contains("\"status\": \"ok\""));
    assert!(new_json.contains("\"title\": \"Revenue Calculator\""));
    assert!(new_json.contains("\"attested\": true"));

    // 3. okf info --json
    let info_out = okf()
        .args(["info", bundle_path.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    assert_eq!(info_out.status.code(), Some(0));
    let info_json = String::from_utf8_lossy(&info_out.stdout);
    assert!(info_json.contains("\"okf_version\": \"0.2\""));
    assert!(info_json.contains("\"concepts_count\": 2"));
    assert!(info_json.contains("\"computations_count\": 1"));

    // 4. okf validate --json
    let val_out = okf()
        .args(["validate", bundle_path.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    assert_eq!(val_out.status.code(), Some(0));
    let val_json = String::from_utf8_lossy(&val_out.stdout);
    assert!(val_json.contains("\"conformant\": true"));
    assert!(val_json.contains("\"error_count\": 0"));

    // 5. okf lint --json
    let lint_out = okf()
        .args(["lint", bundle_path.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    // Scaffolding placeholder resources will have lint warnings
    let lint_json = String::from_utf8_lossy(&lint_out.stdout);
    assert!(lint_json.contains("\"okf_version\": \"0.2\""));
    assert!(lint_json.contains("\"concepts_count\": 2"));

    // 6. okf trust --json
    let trust_out = okf()
        .args(["trust", bundle_path.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    assert_eq!(trust_out.status.code(), Some(0));
    let trust_json = String::from_utf8_lossy(&trust_out.stdout);
    assert!(trust_json.contains("\"summary\":"));
    assert!(trust_json.contains("\"concepts\":"));

    // 7. okf computations --json
    let comp_out = okf()
        .args(["computations", bundle_path.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    assert_eq!(comp_out.status.code(), Some(0));
    let comp_json = String::from_utf8_lossy(&comp_out.stdout);
    assert!(comp_json.contains("\"computations_count\": 1"));
    assert!(comp_json.contains("\"id\": \"computations/revenue\""));

    // 8. okf links --json
    let links_out = okf()
        .args(["links", bundle_path.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    assert_eq!(links_out.status.code(), Some(0));
    let links_json = String::from_utf8_lossy(&links_out.stdout);
    assert!(links_json.contains("\"broken_count\": 0"));

    // 9. okf index --json
    let index_out = okf()
        .args(["index", bundle_path.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    assert_eq!(index_out.status.code(), Some(0));
    let index_json = String::from_utf8_lossy(&index_out.stdout);
    assert!(index_json.contains("\"regenerated_count\":"));

    // 10. okf graph --json
    let graph_out = okf()
        .args(["graph", bundle_path.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    assert_eq!(graph_out.status.code(), Some(0));
    let graph_json = String::from_utf8_lossy(&graph_out.stdout);
    assert!(graph_json.contains("\"concepts\":"));

    // 11. okf parse --json
    let parse_out = okf()
        .args([
            "parse",
            bundle_path
                .join("computations/revenue.md")
                .to_str()
                .unwrap(),
            "--json",
        ])
        .output()
        .unwrap();
    assert_eq!(parse_out.status.code(), Some(0));
    let parse_json = String::from_utf8_lossy(&parse_out.stdout);
    assert!(parse_json.contains("\"conformant\": true"));
    assert!(parse_json.contains("\"type\": \"Attested Computation\""));
    assert!(parse_json.contains("\"attested_computation\":"));

    // 12. okf diff --json
    let bundle_b = tmp.path().join("json_bundle_b");
    okf()
        .args(["init", bundle_b.to_str().unwrap(), "--bare"])
        .status()
        .unwrap();
    let diff_out = okf()
        .args([
            "diff",
            bundle_path.to_str().unwrap(),
            bundle_b.to_str().unwrap(),
            "--json",
        ])
        .output()
        .unwrap();
    assert_eq!(diff_out.status.code(), Some(0));
    let diff_json = String::from_utf8_lossy(&diff_out.stdout);
    assert!(diff_json.contains("\"bundle_a\":"));
    assert!(diff_json.contains("\"bundle_b\":"));
    assert!(diff_json.contains("\"changes_count\":"));
}

#[test]
fn cli_index_and_fmt_do_not_oscillate_on_subdirectories() {
    let tmp = TempDir::new();
    let bundle_path = tmp.path().join("oscillation_bundle");

    // Initialize bundle
    let init_status = okf()
        .args(["init", bundle_path.to_str().unwrap()])
        .status()
        .unwrap();
    assert_eq!(code(init_status), 0);

    // Create a concept inside a subdirectory
    let sub_concept = bundle_path.join("metrics/revenue.md");
    std::fs::create_dir_all(sub_concept.parent().unwrap()).unwrap();
    std::fs::write(
        &sub_concept,
        "---\ntype: Metric\ntitle: Revenue\ndescription: Recognized revenue.\nstatus: stable\n---\n\n# Revenue\n\nRevenue concept body.\n",
    )
    .unwrap();

    // 1. Run okf index to generate subdirectory index.md
    let idx_output1 = okf()
        .args(["index", bundle_path.to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(idx_output1.status.code(), Some(0));

    let sub_index_path = bundle_path.join("metrics/index.md");
    assert!(sub_index_path.exists());
    let sub_index_content1 = std::fs::read_to_string(&sub_index_path).unwrap();
    assert!(!sub_index_content1.starts_with("---"));

    // 2. okf fmt --check should pass cleanly without flagging subdirectory index.md
    let fmt_check1 = okf()
        .args(["fmt", bundle_path.to_str().unwrap(), "--check"])
        .output()
        .unwrap();
    assert_eq!(
        fmt_check1.status.code(),
        Some(0),
        "fmt --check should pass after index: {}",
        String::from_utf8_lossy(&fmt_check1.stderr)
    );

    // 3. okf fmt -w should keep subdirectory index.md unchanged
    let fmt_write = okf()
        .args(["fmt", bundle_path.to_str().unwrap(), "-w"])
        .output()
        .unwrap();
    assert_eq!(fmt_write.status.code(), Some(0));
    let sub_index_content2 = std::fs::read_to_string(&sub_index_path).unwrap();
    assert_eq!(sub_index_content1, sub_index_content2);

    // 4. Running okf index again should not alter content
    let idx_output2 = okf()
        .args(["index", bundle_path.to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(idx_output2.status.code(), Some(0));
    let sub_index_content3 = std::fs::read_to_string(&sub_index_path).unwrap();
    assert_eq!(sub_index_content1, sub_index_content3);

    // 5. okf fmt --check still passes cleanly
    let fmt_check2 = okf()
        .args(["fmt", bundle_path.to_str().unwrap(), "--check"])
        .output()
        .unwrap();
    assert_eq!(fmt_check2.status.code(), Some(0));
}

#[test]
fn cli_info_unquoted_okf_version() {
    let tmp = TempDir::new();
    tmp.write(
        "index.md",
        "---\nokf_version: 0.2\n---\n\n# Test Bundle\n\n* [Revenue](revenue.md)\n",
    );
    tmp.write(
        "revenue.md",
        "---\ntype: Metric\ntitle: Revenue\ndescription: Revenue metric.\ngenerated:\n  by: human:alice\n  at: 2026-01-01T00:00:00Z\nverified:\n  - by: human:bob\n    at: 2026-01-02T00:00:00Z\n---\n\n# Revenue\n\nRevenue.\n",
    );

    // Text info output
    let text_out = okf()
        .args(["info", tmp.path().to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(text_out.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&text_out.stdout);
    assert!(
        stdout.contains("okf_version: 0.2"),
        "stdout should contain 'okf_version: 0.2', got:\n{stdout}"
    );

    // JSON info output
    let json_out = okf()
        .args(["info", tmp.path().to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    assert_eq!(json_out.status.code(), Some(0));
    let json_str = String::from_utf8_lossy(&json_out.stdout);
    let val: serde_json::Value = serde_json::from_str(&json_str).unwrap();
    assert_eq!(val["okf_version"].as_str(), Some("0.2"));
}

#[test]
fn cli_single_file_validate_and_lint() {
    let tmp = TempDir::new();
    tmp.write(
        "index.md",
        "---\nokf_version: \"0.2\"\n---\n\n# Test Bundle\n\n* [Austria](member-states/austria.md)\n",
    );
    let austria_path = tmp.write(
        "member-states/austria.md",
        "---\ntype: MemberState\ntitle: Austria\ndescription: Republic of Austria.\ngenerated:\n  by: human:alice\n  at: 2026-01-01T00:00:00Z\nverified:\n  - by: human:bob\n    at: 2026-01-02T00:00:00Z\n---\n\n# Austria\n\nAustria is a member state.\n",
    );

    // Validate single file by exact path
    let val_out = okf()
        .args(["validate", austria_path.to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(val_out.status.code(), Some(0));
    let val_stdout = String::from_utf8_lossy(&val_out.stdout);
    assert!(val_stdout.contains("1 concept(s)"), "got:\n{val_stdout}");
    assert!(val_stdout.contains("conformant with OKF v0.2"));

    // Lint single file by exact path
    let lint_out = okf()
        .args(["lint", austria_path.to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(lint_out.status.code(), Some(0));
    let lint_stdout = String::from_utf8_lossy(&lint_out.stdout);
    assert!(lint_stdout.contains("1 concept(s)"), "got:\n{lint_stdout}");
    assert!(lint_stdout.contains("clean lint"));

    // Validate without .md extension
    let path_no_md = austria_path.with_extension("");
    let val_no_md = okf()
        .args(["validate", path_no_md.to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(val_no_md.status.code(), Some(0));

    // Validate single file JSON
    let val_json_out = okf()
        .args(["validate", austria_path.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    assert_eq!(val_json_out.status.code(), Some(0));
    let val_json: serde_json::Value = serde_json::from_slice(&val_json_out.stdout).unwrap();
    assert_eq!(val_json["concepts_count"].as_u64(), Some(1));
    assert_eq!(val_json["conformant"].as_bool(), Some(true));
}

#[test]
fn cli_uncited_source_is_warning_and_fails_lint() {
    let tmp = TempDir::new();
    tmp.write(
        "index.md",
        "---\nokf_version: \"0.2\"\n---\n\n# Bundle\n\n* [Revenue](revenue.md)\n",
    );
    tmp.write(
        "revenue.md",
        "---\ntype: Metric\ntitle: Revenue\ndescription: Revenue metric.\ngenerated:\n  by: human:alice\n  at: 2026-01-01T00:00:00Z\nverified:\n  - by: human:bob\n    at: 2026-01-02T00:00:00Z\nsources:\n  - id: sec-10k\n    resource: https://sec.gov\n---\n\n# Revenue\n\nBody without footnote citation.\n",
    );

    let lint_out = okf()
        .args(["lint", tmp.path().to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(
        lint_out.status.code(),
        Some(65),
        "lint with uncited source must exit with code 65 (EX_DATAERR)"
    );
    let stdout = String::from_utf8_lossy(&lint_out.stdout);
    assert!(stdout.contains("[warning]"), "got:\n{stdout}");
    assert!(stdout.contains("[L5]"), "got:\n{stdout}");
    assert!(stdout.contains("1 lint warning(s)"), "got:\n{stdout}");
}
