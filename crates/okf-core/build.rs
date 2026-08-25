use std::path::PathBuf;
use std::process::{Command, Stdio};

fn main() {
    // Prevent recursive invocations or execution in CI/docs environments where redundant.
    if std::env::var("OKF_SKIP_BUILD_CHECKS").is_ok() || std::env::var("DOCS_RS").is_ok() {
        return;
    }

    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let workspace_root = match manifest_dir.parent().and_then(|p| p.parent()) {
        Some(root) => root.to_path_buf(),
        None => return,
    };

    // Only run these workspace-level sanity checks in local git checkouts.
    if !workspace_root.join(".git").exists() {
        return;
    }

    // Ensure cargo watches files across the workspace so incremental builds know when to re-run.
    println!(
        "cargo:rerun-if-changed={}",
        workspace_root.join("crates").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        workspace_root.join("Cargo.toml").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        workspace_root.join("Cargo.lock").display()
    );

    let target_dir = workspace_root.join("target").join(".check_cache");

    // 1. Auto-format all code
    let _ = Command::new("cargo")
        .current_dir(&workspace_root)
        .env("OKF_SKIP_BUILD_CHECKS", "1")
        .args(["fmt"])
        .status();

    // Verify formatting
    let fmt_check = Command::new("cargo")
        .current_dir(&workspace_root)
        .env("OKF_SKIP_BUILD_CHECKS", "1")
        .args(["fmt", "--check"])
        .status();

    if let Ok(status) = fmt_check
        && !status.success()
    {
        eprintln!("error: code formatting check failed; run 'cargo fmt'");
        std::process::exit(1);
    }

    // 2. Auto-fix clippy issues where possible
    let _ = Command::new("cargo")
        .current_dir(&workspace_root)
        .env("OKF_SKIP_BUILD_CHECKS", "1")
        .args([
            "clippy",
            "--fix",
            "--allow-dirty",
            "--allow-staged",
            "--workspace",
            "--all-targets",
            "--target-dir",
            target_dir.to_str().unwrap(),
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();

    // Check for remaining clippy warnings/errors
    let clippy_status = Command::new("cargo")
        .current_dir(&workspace_root)
        .env("OKF_SKIP_BUILD_CHECKS", "1")
        .args([
            "clippy",
            "--workspace",
            "--all-targets",
            "--target-dir",
            target_dir.to_str().unwrap(),
            "--",
            "-D",
            "warnings",
        ])
        .status();

    match clippy_status {
        Ok(status) if !status.success() => {
            std::process::exit(status.code().unwrap_or(1));
        }
        Err(err) => {
            eprintln!("error: failed to execute cargo clippy: {err}");
            std::process::exit(1);
        }
        _ => {}
    }

    // 3. Run test suite
    let test_status = Command::new("cargo")
        .current_dir(&workspace_root)
        .env("OKF_SKIP_BUILD_CHECKS", "1")
        .args([
            "test",
            "--workspace",
            "--target-dir",
            target_dir.to_str().unwrap(),
        ])
        .status();

    match test_status {
        Ok(status) if !status.success() => {
            std::process::exit(status.code().unwrap_or(1));
        }
        Err(err) => {
            eprintln!("error: failed to execute cargo test: {err}");
            std::process::exit(1);
        }
        _ => {}
    }
}
