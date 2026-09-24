//! The global `--ignore <SOURCE>` flag and the always-on `.okfignore`.
//!
//! Every scenario drives the real binary so the flag's placement, exit codes,
//! and cross-subcommand consistency are what a user would see.

mod common;

use common::TempDir;
use std::path::Path;
use std::process::{Command, Output};

const EX_NOINPUT: i32 = 66;

/// A conformant concept with canonical formatting, so `fmt --check` is clean.
const CLEAN: &str = "---\ntype: Metric\ntitle: T\ndescription: d\ngenerated:\n  at: \"2026-01-01T00:00:00Z\"\n---\n\n# T\n\nBody.\n";
/// A concept whose frontmatter keys are out of canonical order, so
/// `fmt --check` flags it.
const UNFORMATTED: &str = "---\ntitle: T\ntype: Metric\ndescription: d\ngenerated:\n  at: 2026-01-01T00:00:00Z\n---\n\n# T\n\nBody.\n";

fn okf_in(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_okf"))
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap()
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

/// `okf info --json` concept count for `args` run inside `dir`.
fn concept_count(dir: &Path, extra: &[&str]) -> u64 {
    let mut args = vec!["info", "--json"];
    args.extend_from_slice(extra);
    args.push(".");
    let out = okf_in(dir, &args);
    assert!(out.status.success(), "{}", stderr(&out));
    let v: serde_json::Value = serde_json::from_str(&stdout(&out)).unwrap();
    v["concepts_count"].as_u64().unwrap()
}

fn bundle() -> TempDir {
    let tmp = TempDir::new();
    tmp.write("index.md", "---\nokf_version: \"0.2\"\n---\n\n# B\n");
    tmp.write("a.md", CLEAN);
    tmp.write("drafts/d.md", CLEAN);
    tmp.write("sub/s.md", CLEAN);
    tmp
}

#[test]
fn okfignore_applies_with_no_flag() {
    let tmp = bundle();
    assert_eq!(concept_count(tmp.path(), &[]), 3);
    tmp.write(".okfignore", "drafts/\n");
    assert_eq!(concept_count(tmp.path(), &[]), 2);
}

/// A relative target with no enclosing `index.md` falls through to the
/// single-file load instead of cycling between `""` and `"."` forever.
#[test]
fn relative_target_outside_a_bundle_terminates() {
    let tmp = TempDir::new();
    tmp.write("drafts/a.md", CLEAN);
    let out = okf_in(tmp.path(), &["validate", "--json", "drafts/a.md"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let v: serde_json::Value = serde_json::from_str(&stdout(&out)).unwrap();
    assert_eq!(v["concepts_count"].as_u64(), Some(1));
}

#[test]
fn flag_after_and_before_subcommand_are_equivalent() {
    let tmp = bundle();
    tmp.write(".gitignore", "drafts/\n");
    // Not honoured without the flag.
    assert_eq!(concept_count(tmp.path(), &[]), 3);

    let after = okf_in(
        tmp.path(),
        &["validate", "--ignore", ".gitignore", "--json", "."],
    );
    let before = okf_in(
        tmp.path(),
        &["--ignore", ".gitignore", "validate", "--json", "."],
    );
    assert!(after.status.success(), "{}", stderr(&after));
    assert_eq!(stdout(&after), stdout(&before));
    let v: serde_json::Value = serde_json::from_str(&stdout(&after)).unwrap();
    assert_eq!(v["concepts_count"], 2, "{v}");
}

#[test]
fn multiple_sources_apply_in_order() {
    let tmp = bundle();
    tmp.write(".a-ignore", "a.md\n");
    tmp.write(".b-ignore", "!a.md\n");
    // Only `.a-ignore`: a.md is gone.
    assert_eq!(concept_count(tmp.path(), &["--ignore", ".a-ignore"]), 2);
    // `.b-ignore` after `.a-ignore` re-includes it.
    assert_eq!(
        concept_count(
            tmp.path(),
            &["--ignore", ".a-ignore", "--ignore", ".b-ignore"]
        ),
        3
    );
    // Reverse order: `.a-ignore` wins.
    assert_eq!(
        concept_count(
            tmp.path(),
            &["--ignore", ".b-ignore", "--ignore", ".a-ignore"]
        ),
        2
    );
}

#[test]
fn name_source_is_discovered_per_directory() {
    let tmp = bundle();
    tmp.write("sub/.myignore", "*.md\n");
    assert_eq!(concept_count(tmp.path(), &["--ignore", ".myignore"]), 2);
}

#[test]
fn explicit_path_is_anchored_to_its_directory() {
    let outer = TempDir::new();
    outer.write("shared.ignore", "bundle/drafts/\n");
    outer.write("bundle/index.md", "---\nokf_version: \"0.2\"\n---\n\n# B\n");
    outer.write("bundle/a.md", CLEAN);
    outer.write("bundle/drafts/d.md", CLEAN);
    // A sibling bundle is *not* affected: the rule names `bundle/drafts/`.
    outer.write("other/drafts/d.md", CLEAN);
    outer.write("other/index.md", "---\nokf_version: \"0.2\"\n---\n\n# O\n");

    let root = outer.path().join("bundle");
    assert_eq!(concept_count(&root, &["--ignore", "../shared.ignore"]), 1);
    let other = outer.path().join("other");
    assert_eq!(concept_count(&other, &["--ignore", "../shared.ignore"]), 1);
}

#[test]
fn missing_explicit_path_is_ex_noinput() {
    let tmp = bundle();
    let out = okf_in(tmp.path(), &["validate", "--ignore", "./nope", "."]);
    assert_eq!(out.status.code(), Some(EX_NOINPUT));
    assert!(stderr(&out).contains("./nope"), "{}", stderr(&out));
}

#[test]
fn missing_name_is_silent() {
    let tmp = bundle();
    let plain = okf_in(tmp.path(), &["validate", "--json", "."]);
    let with = okf_in(
        tmp.path(),
        &["validate", "--ignore", ".noignore", "--json", "."],
    );
    assert!(with.status.success(), "{}", stderr(&with));
    assert_eq!(stdout(&plain), stdout(&with));
}

#[test]
fn dockerignore_is_root_anchored() {
    let tmp = bundle();
    tmp.write("README.md", CLEAN);
    tmp.write("sub/README.md", CLEAN);
    tmp.write(".dockerignore", "README.md\n");
    // Bare name under Docker rules matches only at the root.
    assert_eq!(concept_count(tmp.path(), &["--ignore", ".dockerignore"]), 4);
    // The same content as a gitignore name would match both.
    tmp.write(".gitignore", "README.md\n");
    assert_eq!(concept_count(tmp.path(), &["--ignore", ".gitignore"]), 3);
}

#[test]
fn explicit_single_file_target_bypasses_ignore() {
    let tmp = bundle();
    tmp.write(".okfignore", "drafts/\n");
    let out = okf_in(tmp.path(), &["validate", "--json", "drafts/d.md"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let v: serde_json::Value = serde_json::from_str(&stdout(&out)).unwrap();
    assert_eq!(v["concepts_count"], 1, "{v}");
}

#[test]
fn validate_and_fmt_agree_on_the_file_set() {
    let tmp = bundle();
    tmp.write("vendor/v.md", UNFORMATTED);
    tmp.write(".okfignore", "vendor/\n");

    assert_eq!(concept_count(tmp.path(), &[]), 3);

    let out = okf_in(tmp.path(), &["fmt", "--check", "--json", "."]);
    assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));
    let v: serde_json::Value = serde_json::from_str(&stdout(&out)).unwrap();
    assert_eq!(v["clean"], true, "{v}");
    // index.md + 3 concepts; vendor/v.md is not counted.
    assert_eq!(v["total_files"], 4, "{v}");
}

#[test]
fn validate_and_fmt_skip_the_same_defaults() {
    let tmp = bundle();
    tmp.write(".notes/n.md", UNFORMATTED);
    tmp.write("node_modules/pkg/README.md", UNFORMATTED);
    tmp.write("target/out.md", UNFORMATTED);
    // Every command skips hidden dirs, `target`, and `node_modules` by
    // default: fmt is clean and info/validate see only the 3 base concepts.
    let out = okf_in(tmp.path(), &["fmt", "--check", "--json", "."]);
    let v: serde_json::Value = serde_json::from_str(&stdout(&out)).unwrap();
    assert_eq!(v["clean"], true, "{v}");
    assert_eq!(concept_count(tmp.path(), &[]), 3);

    // A `!` rule in .okfignore re-includes one of them, for every command.
    tmp.write(".okfignore", "!.notes/\n");
    assert_eq!(concept_count(tmp.path(), &[]), 4);
    let out = okf_in(tmp.path(), &["validate", "--json", "."]);
    let v: serde_json::Value = serde_json::from_str(&stdout(&out)).unwrap();
    assert_eq!(v["concepts_count"], 4, "{v}");
    let out = okf_in(tmp.path(), &["fmt", "--check", "--json", "."]);
    let v: serde_json::Value = serde_json::from_str(&stdout(&out)).unwrap();
    assert_eq!(v["clean"], false, "{v}");
    let unformatted: Vec<&str> = v["unformatted"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p.as_str().unwrap())
        .collect();
    assert!(unformatted.iter().any(|p| p.contains(".notes")), "{v}");
    assert!(
        !unformatted
            .iter()
            .any(|p| p.contains("node_modules") || p.contains("target")),
        "{v}"
    );
}

#[test]
fn index_and_fix_respect_okfignore() {
    let tmp = bundle();
    tmp.write(".okfignore", "drafts/\n");
    tmp.write("drafts/untitled.md", "---\ntype: Concept\n---\nBody.\n");

    let out = okf_in(tmp.path(), &["index", "."]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(!tmp.path().join("drafts/index.md").exists());
    assert!(!tmp.read("index.md").contains("drafts"));

    let before = tmp.read("drafts/untitled.md");
    let out = okf_in(tmp.path(), &["validate", "--fix", "."]);
    assert!(
        out.status.code().is_some(),
        "validate --fix crashed: {}",
        stderr(&out)
    );
    assert_eq!(
        tmp.read("drafts/untitled.md"),
        before,
        "ignored file was modified"
    );
}

#[cfg(unix)]
#[test]
fn a_target_named_through_an_in_tree_symlink_is_the_concept_it_points_to() {
    let tmp = bundle();
    std::os::unix::fs::symlink(tmp.path().join("drafts"), tmp.path().join("link")).unwrap();

    // The loader takes the target as the walked `drafts/d.md`, so the command
    // must report that concept rather than finding none under `link/d`.
    let out = okf_in(tmp.path(), &["validate", "--json", "link/d.md"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let v: serde_json::Value = serde_json::from_str(&stdout(&out)).unwrap();
    assert_eq!(v["concepts_count"].as_u64().unwrap(), 1, "{v}");
}

#[test]
fn help_lists_the_flag_globally() {
    let tmp = bundle();
    for args in [&["--help"][..], &["validate", "--help"], &["fmt", "--help"]] {
        let out = okf_in(tmp.path(), args);
        let text = stdout(&out);
        assert!(text.contains("--ignore <SOURCE>"), "{args:?}: {text}");
    }
    let out = okf_in(tmp.path(), &["validate", "--help"]);
    let text = stdout(&out);
    assert!(text.contains(".okfignore"), "{text}");
}

#[test]
fn a_rule_matching_an_index_file_warns_and_is_overridden() {
    let tmp = bundle();
    tmp.write(".okfignore", "index.md\n");

    for args in [
        &["info", "."][..],
        &["index", "."],
        &["fmt", "--check", "."],
    ] {
        let out = okf_in(tmp.path(), args);
        let err = stderr(&out);
        assert_eq!(
            err.matches("warning: an ignore rule matches").count(),
            3,
            "{args:?}: {err}"
        );
        assert!(err.contains("sub/index.md"), "{args:?}: {err}");
    }
    assert!(tmp.read("index.md").contains("okf_version"));
    assert!(tmp.read("index.md").contains("(sub/index.md)"));

    let out = okf_in(tmp.path(), &["info", "--json", "."]);
    let v: serde_json::Value = serde_json::from_str(&stdout(&out)).unwrap();
    assert_eq!(v["okf_version"], "0.2", "{v}");
}
