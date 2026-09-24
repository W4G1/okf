//! Parity with `git check-ignore`.
//!
//! Builds a fixture tree with nested `.gitignore` files exercising the whole
//! grammar, asks git which paths it would ignore, and asserts the walker
//! reaches exactly the complement. Skips (passes) when `git` is not on `PATH`
//! so CI images without git still go green.

mod common;

use common::TempDir;
use okf_core::ignore::IgnoreConfig;
use okf_core::walk::{WalkOptions, walk_markdown};
use std::collections::BTreeSet;
use std::process::{Command, Stdio};

const ROOT_GITIGNORE: &str = "\
# comment line
*.tmp.md
!keep.tmp.md
/anchored.md
build/
**/generated
logs/**
docs/**/z.md
v?.md
[a-c]lass.md
[!x]neg.md
\\#hash.md
\\!bang.md
trailing.md
escaped\\ space.md
nested/deep/ignored.md
star*.md
a**b.md
dir-only/
/rooted-dir/
node_modules
target
";

const SUB_GITIGNORE: &str = "\
# sub-level rules override root ones
!*.tmp.md
local.md
/only-here.md
inner/
!logs
";

const SUB_INNER_GITIGNORE: &str = "\
!inner-keep.md
";

/// Every markdown file in the fixture, relative to the repo root.
const FILES: &[&str] = &[
    "a.tmp.md",
    "keep.tmp.md",
    "anchored.md",
    "sub/anchored.md",
    "build/x.md",
    "build/y/z.md",
    "generated/g.md",
    "a/generated/g.md",
    "a/b/generated/g.md",
    "generated.md",
    "logs/l.md",
    "logs/deep/l.md",
    "logs.md",
    "docs/z.md",
    "docs/a/z.md",
    "docs/a/b/z.md",
    "docs/y.md",
    "v1.md",
    "v12.md",
    "alass.md",
    "class.md",
    "dlass.md",
    "aneg.md",
    "xneg.md",
    "#hash.md",
    "!bang.md",
    "trailing.md",
    "escaped space.md",
    "nested/deep/ignored.md",
    "nested/deep/other.md",
    "nested/ignored.md",
    "starfish.md",
    "star.md",
    // `Case.md` vs `case.md` is deliberately absent: they collapse into one
    // file on case-insensitive filesystems. Case sensitivity is a unit test.
    "ab.md",
    "axyzb.md",
    "dir-only/f.md",
    "dir-only.md",
    "rooted-dir/f.md",
    "sub/rooted-dir/f.md",
    "node_modules/pkg/README.md",
    "target/debug/notes.md",
    "sub/b.tmp.md",
    "sub/local.md",
    "sub/x/local.md",
    "sub/only-here.md",
    "sub/x/only-here.md",
    "sub/inner/i.md",
    "sub/inner/inner-keep.md",
    "sub/logs/l.md",
    "sub/plain.md",
    "plain.md",
];

fn git_available() -> bool {
    Command::new("git")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

fn git(repo: &std::path::Path, args: &[&str]) -> std::process::Output {
    Command::new("git")
        .args(args)
        .current_dir(repo)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("HOME", repo)
        .output()
        .expect("spawn git")
}

#[test]
fn walker_matches_git_check_ignore() {
    if !git_available() {
        eprintln!("skipping: git not on PATH");
        return;
    }

    let tmp = TempDir::new();
    let repo = tmp.path();
    tmp.write(".gitignore", ROOT_GITIGNORE);
    tmp.write("sub/.gitignore", SUB_GITIGNORE);
    tmp.write("sub/inner/.gitignore", SUB_INNER_GITIGNORE);
    for f in FILES {
        tmp.write(f, "---\ntype: X\n---\n");
    }

    let init = git(repo, &["init", "-q"]);
    assert!(
        init.status.success(),
        "{}",
        String::from_utf8_lossy(&init.stderr)
    );
    // Case-sensitive matching regardless of the host filesystem's default.
    git(repo, &["config", "core.ignorecase", "false"]);

    // Ask git. `check-ignore` exits 1 when nothing matched, which is fine.
    let mut child = Command::new("git")
        .args(["check-ignore", "--stdin", "--no-index"])
        .current_dir(repo)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("HOME", repo)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn git check-ignore");
    {
        use std::io::Write as _;
        let mut stdin = child.stdin.take().unwrap();
        for f in FILES {
            writeln!(stdin, "{f}").unwrap();
        }
    }
    let out = child.wait_with_output().unwrap();
    assert!(
        out.status.code().is_some_and(|c| c == 0 || c == 1),
        "git check-ignore failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let git_ignored: BTreeSet<String> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::to_string)
        .collect();
    let git_kept: BTreeSet<String> = FILES
        .iter()
        .map(|s| (*s).to_string())
        .filter(|f| !git_ignored.contains(f))
        .collect();

    // Ask the walker.
    let cfg = IgnoreConfig::new().with_name(".gitignore");
    let ours: BTreeSet<String> = walk_markdown(repo, &WalkOptions::new(&cfg))
        .unwrap()
        .iter()
        .map(|p| {
            p.strip_prefix(repo)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/")
        })
        .collect();

    let only_git_keeps: Vec<_> = git_kept.difference(&ours).collect();
    let only_we_keep: Vec<_> = ours.difference(&git_kept).collect();
    assert!(
        only_git_keeps.is_empty() && only_we_keep.is_empty(),
        "divergence from git\n  git keeps but we ignore: {only_git_keeps:?}\n  we keep but git ignores: {only_we_keep:?}\n  git ignored: {git_ignored:?}"
    );
    // Sanity: the fixture exercises both outcomes.
    assert!(git_ignored.len() > 20, "{git_ignored:?}");
    assert!(git_kept.len() > 10, "{git_kept:?}");
}
