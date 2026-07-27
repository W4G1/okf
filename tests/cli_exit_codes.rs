//! Exit-code conformance for the `okf` CLI.
//!
//! The CLI follows the `sysexits.h` convention so CI can distinguish a missing
//! bundle from a malformed one without parsing stderr. The codes exercised
//! here are: `EX_USAGE` (2) for a bad command line, `EX_DATAERR` (65) for
//! incorrect input data, and `EX_NOINPUT` (66) for an input that could not be
//! opened.

mod common;

use common::TempDir;
use std::process::{Command, ExitStatus};

const EX_USAGE: i32 = 2;
const EX_DATAERR: i32 = 65;
const EX_NOINPUT: i32 = 66;

fn okf() -> Command {
    Command::new(env!("CARGO_BIN_EXE_okf"))
}

fn code(status: ExitStatus) -> i32 {
    status.code().unwrap_or(0)
}

/// A minimal conformant bundle: one concept that links to another existing
/// concept, so `validate` and `lint` both succeed.
fn good_bundle() -> TempDir {
    let tmp = TempDir::new();
    tmp.write("index.md", "---\nokf_version: \"0.2\"\n---\n\n# Mini\n");
    tmp.write(
        "metrics/revenue.md",
        "---\ntype: Metric\ntitle: Revenue\ngenerated:\n  at: 2026-01-01T00:00:00Z\n---\n\nRevenue is money.\n",
    );
    tmp
}

#[test]
fn no_args_is_usage_error() {
    let status = okf().status().unwrap();
    assert_eq!(code(status), EX_USAGE);
}

#[test]
fn unknown_subcommand_is_usage_error() {
    let status = okf().arg("bogus").status().unwrap();
    assert_eq!(code(status), EX_USAGE);
}

#[test]
fn help_and_version_succeed() {
    for flag in ["--help", "-h", "--version", "-V"] {
        let status = okf().arg(flag).status().unwrap();
        assert_eq!(code(status), 0, "flag {flag} should succeed");
    }
}

#[test]
fn validate_missing_bundle_is_no_input() {
    let status = okf()
        .args(["validate", "/no/such/bundle/here"])
        .status()
        .unwrap();
    assert_eq!(code(status), EX_NOINPUT);
}

#[test]
fn validate_conformant_bundle_succeeds() {
    let tmp = good_bundle();
    let status = okf()
        .args(["validate", tmp.path().to_str().unwrap()])
        .status()
        .unwrap();
    assert_eq!(code(status), 0);
}

#[test]
fn validate_nonconformant_bundle_is_data_error() {
    let tmp = TempDir::new();
    tmp.write("index.md", "---\nokf_version: \"0.2\"\n---\n\n# Mini\n");
    // A concept with no `type` fails §11 conformance.
    tmp.write(
        "metrics/revenue.md",
        "---\ntitle: Revenue\n---\n\nRevenue is money.\n",
    );
    let status = okf()
        .args(["validate", tmp.path().to_str().unwrap()])
        .status()
        .unwrap();
    assert_eq!(code(status), EX_DATAERR);
}

#[test]
fn validate_bad_today_is_usage_error() {
    let tmp = good_bundle();
    let status = okf()
        .args([
            "validate",
            tmp.path().to_str().unwrap(),
            "--today",
            "not-a-date",
        ])
        .status()
        .unwrap();
    assert_eq!(code(status), EX_USAGE);
}

#[test]
fn validate_today_without_value_is_usage_error() {
    let tmp = good_bundle();
    let status = okf()
        .args(["validate", tmp.path().to_str().unwrap(), "--today"])
        .status()
        .unwrap();
    assert_eq!(code(status), EX_USAGE);
}

#[test]
fn lint_with_warnings_is_data_error() {
    let tmp = good_bundle();
    // A dangling link triggers a lint warning, so the bundle is not clean.
    tmp.write(
        "metrics/costs.md",
        "---\ntype: Metric\ntitle: Costs\n---\n\nSee [ghost](ghost.md).\n",
    );
    let status = okf()
        .args(["lint", tmp.path().to_str().unwrap()])
        .status()
        .unwrap();
    assert_eq!(code(status), EX_DATAERR);
}

#[test]
fn parse_missing_file_is_no_input() {
    let status = okf().args(["parse", "/no/such/file.md"]).status().unwrap();
    assert_eq!(code(status), EX_NOINPUT);
}

#[test]
fn parse_malformed_file_is_data_error() {
    let tmp = TempDir::new();
    let path = tmp.write("bad.md", "---\ntype: Metric\n");
    let status = okf()
        .args(["parse", path.to_str().unwrap()])
        .status()
        .unwrap();
    assert_eq!(code(status), EX_DATAERR);
}

#[test]
fn fmt_missing_file_is_no_input() {
    let status = okf().args(["fmt", "/no/such/file.md"]).status().unwrap();
    assert_eq!(code(status), EX_NOINPUT);
}

#[test]
fn index_missing_bundle_is_no_input() {
    let status = okf()
        .args(["index", "/no/such/bundle/here"])
        .status()
        .unwrap();
    assert_eq!(code(status), EX_NOINPUT);
}

#[test]
fn diff_missing_second_bundle_is_usage_error() {
    let tmp = good_bundle();
    let status = okf()
        .args(["diff", tmp.path().to_str().unwrap()])
        .status()
        .unwrap();
    assert_eq!(code(status), EX_USAGE);
}

#[test]
fn diff_missing_bundle_is_no_input() {
    let tmp = good_bundle();
    let status = okf()
        .args(["diff", "/no/such/bundle/here", tmp.path().to_str().unwrap()])
        .status()
        .unwrap();
    assert_eq!(code(status), EX_NOINPUT);
}
