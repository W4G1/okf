//! Exact-time evaluation, date compatibility, and command-line validation.

#![cfg(feature = "validator")]

mod common;

use clap::Parser;
use common::TempDir;
use okf::cli::Cli;
use serde_json::Value;
use std::process::Command;

fn fixture(stale_after: &str) -> TempDir {
    let tmp = TempDir::new();
    tmp.write("index.md", "---\nokf_version: \"0.3\"\n---\n\n# Root\n");
    tmp.write(
        "a.md",
        &format!("---\ntype: Note\ntitle: A\nstale_after: \"{stale_after}\"\n---\n\n# A\n"),
    );
    tmp
}

fn evaluate(tmp: &TempDir, command: &str, flags: &[&str]) -> Value {
    let target = if command == "parse" {
        tmp.path().join("a.md")
    } else {
        tmp.path().to_path_buf()
    };
    let out = Command::new(env!("CARGO_BIN_EXE_okf"))
        .arg(command)
        .arg(target)
        .arg("--json")
        .args(flags)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{command}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}

fn is_stale(command: &str, value: &Value) -> bool {
    match command {
        "validate" => value["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|diagnostic| {
                diagnostic["message"]
                    .as_str()
                    .unwrap()
                    .starts_with("stale since ")
            }),
        "info" => value["stale_count"].as_u64().unwrap() != 0,
        "trust" => value["concepts"][0]["stale"].as_bool().unwrap(),
        "parse" => value["stale"].as_bool().unwrap(),
        _ => panic!("unexpected command {command}"),
    }
}

#[test]
fn at_pins_the_exact_instant_across_validation_and_inspection() {
    let tmp = fixture("2026-07-01T10:00:00.500000000Z");
    for command in ["validate", "info", "trust", "parse"] {
        for (at, expected) in [
            ("2026-07-01T10:00:00.499999999Z", false),
            ("2026-07-01T10:00:00.500000000Z", true),
            ("2026-07-01T10:00:00.500000001Z", true),
            ("2026-07-01T12:00:00.5+02:00", true),
            ("2026-07-01T05:00:00.5-05:00", true),
            ("2026-07-02T00:00:00.5+14:00", true),
            ("2026-07-01t10:00:00.5z", true),
        ] {
            let value = evaluate(&tmp, command, &["--at", at]);
            assert_eq!(
                is_stale(command, &value),
                expected,
                "{command} --at {at}: {value}"
            );
            assert_eq!(
                evaluate(&tmp, command, &["--at", at]),
                value,
                "pinned output must be reproducible"
            );
        }
    }
}

#[test]
fn today_still_means_midnight_utc() {
    let tmp = fixture("2026-07-01T10:00:00Z");
    for command in ["validate", "info", "trust", "parse"] {
        let day = evaluate(&tmp, command, &["--today", "2026-07-01"]);
        assert!(!is_stale(command, &day), "{command}");
        assert_eq!(
            day,
            evaluate(&tmp, command, &["--at", "2026-07-01T00:00:00Z"])
        );
        assert!(
            is_stale(
                command,
                &evaluate(&tmp, command, &["--today", "2026-07-02"])
            ),
            "{command}"
        );
    }
}

#[test]
fn inspection_without_a_pin_uses_the_current_time() {
    let tmp = fixture("1970-01-01T00:00:00Z");
    for command in ["validate", "info", "trust", "parse"] {
        assert!(
            is_stale(command, &evaluate(&tmp, command, &[])),
            "{command}"
        );
    }
}

#[test]
fn all_date_aware_commands_accept_at_and_reject_ambiguous_or_invalid_pins() {
    let mut commands = vec!["validate", "lint", "info", "trust", "parse"];
    if cfg!(feature = "studio") {
        commands.push("studio");
    }
    for command in commands {
        assert!(
            Cli::try_parse_from([
                "okf",
                command,
                "bundle",
                "--at",
                "2026-07-01T12:30:00+02:00"
            ])
            .is_ok(),
            "{command}"
        );
        for invalid in [
            "not-a-timestamp",
            "2026-07-01",
            "2026-07-01T12:30:00",
            "2026-07-01 12:30:00Z",
            "2026-07-01T12:30Z",
            "2026-07-01T12:30:00+0200",
            "2026-07-01T24:00:00Z",
            "2026-02-30T12:30:00Z",
            "2026-07-01T12:30:00+24:00",
        ] {
            let error =
                Cli::try_parse_from(["okf", command, "bundle", "--at", invalid]).unwrap_err();
            assert_eq!(error.exit_code(), 2, "{command} --at {invalid}");
            assert!(error.to_string().contains("RFC 3339"), "{error}");
        }
        let error = Cli::try_parse_from(["okf", command, "bundle", "--at"]).unwrap_err();
        assert_eq!(error.exit_code(), 2);
        for flags in [
            ["--at", "2026-07-01T12:00:00Z", "--today", "2026-07-01"],
            ["--today", "2026-07-01", "--at", "2026-07-01T12:00:00Z"],
        ] {
            let error = Cli::try_parse_from(["okf", command, "bundle"].into_iter().chain(flags))
                .unwrap_err();
            assert_eq!(
                error.kind(),
                clap::error::ErrorKind::ArgumentConflict,
                "{command}"
            );
        }
    }
}

#[test]
fn at_errors_use_the_cli_usage_exit_code() {
    for flags in [
        vec!["--at", "2026-07-01"],
        vec!["--at"],
        vec!["--at", "2026-07-01T12:00:00Z", "--today", "2026-07-01"],
    ] {
        let out = Command::new(env!("CARGO_BIN_EXE_okf"))
            .arg("validate")
            .args(flags)
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(2));
    }
}
