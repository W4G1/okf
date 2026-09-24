//! Ignore rules as seen through the library API: `Bundle::load`,
//! `Bundle::load_with`, index regeneration, and remediation.

mod common;

use common::TempDir;
use okf_core::bundle::LoadOptions;
use okf_core::ignore::{IgnoreConfig, IgnoreSource};
use okf_core::{Bundle, ConceptId};

const CONCEPT: &str = "---\ntype: Reference\ntitle: T\ndescription: d\n---\n\n# T\n\nProse.\n";

fn ids(bundle: &Bundle) -> Vec<String> {
    bundle.concepts().iter().map(|c| c.id.to_string()).collect()
}

#[test]
fn default_load_honours_okfignore() {
    let tmp = TempDir::new();
    tmp.write(".okfignore", "drafts/\n");
    tmp.write("drafts/a.md", CONCEPT);
    tmp.write("b.md", CONCEPT);

    let bundle = Bundle::load(tmp.path()).unwrap();
    assert_eq!(ids(&bundle), vec!["b"]);
    assert!(!bundle.contains(&ConceptId::parse("drafts/a").unwrap()));
    assert_eq!(bundle.load_options(), &LoadOptions::default());
}

#[test]
fn load_with_extra_source() {
    let tmp = TempDir::new();
    tmp.write(".gitignore", "*.tmp.md\n");
    tmp.write("x.tmp.md", CONCEPT);
    tmp.write("y.md", CONCEPT);

    // Without the source, `.gitignore` is not consulted.
    let plain = Bundle::load(tmp.path()).unwrap();
    assert_eq!(ids(&plain), vec!["x.tmp", "y"]);

    let opts = LoadOptions::with_ignore(IgnoreConfig::new().with_name(".gitignore"));
    let bundle = Bundle::load_with(tmp.path(), &opts).unwrap();
    assert_eq!(ids(&bundle), vec!["y"]);
    assert_eq!(bundle.load_options(), &opts);
}

#[test]
fn default_skips_apply_and_okfignore_reincludes() {
    let tmp = TempDir::new();
    tmp.write(".git/notes.md", CONCEPT);
    tmp.write(".drafts/a.md", CONCEPT);
    tmp.write("node_modules/pkg/README.md", CONCEPT);
    tmp.write("x/target/out.md", CONCEPT);
    tmp.write("b.md", CONCEPT);

    // Hidden directories, `target`, and `node_modules` are skipped by every
    // load, matching what `fmt` and `--fix` always did.
    let bundle = Bundle::load(tmp.path()).unwrap();
    assert_eq!(ids(&bundle), vec!["b"]);

    // A `!` rule in .okfignore re-includes a directory.
    tmp.write(".okfignore", "!.drafts/\n");
    let bundle = Bundle::load(tmp.path()).unwrap();
    assert_eq!(ids(&bundle), vec![".drafts/a", "b"]);
}

#[test]
fn a_rule_matching_the_root_index_is_overridden() {
    let tmp = TempDir::new();
    tmp.write("index.md", "---\nokf_version: \"0.2\"\n---\n\n# B\n");
    tmp.write("a.md", CONCEPT);
    tmp.write(".okfignore", "index.md\n");
    let bundle = Bundle::load(tmp.path()).unwrap();
    assert_eq!(bundle.okf_version(), Some("0.2"));
    assert_eq!(bundle.index_files(), [tmp.path().join("index.md")]);
}

#[test]
fn included_file_outside_the_root_is_skipped() {
    let tmp = TempDir::new();
    tmp.write("a.md", CONCEPT);
    let outside = TempDir::new();
    outside.write("secret.md", CONCEPT);

    // Both temporary directories are siblings, so this climbs out of the
    // bundle while still reading as a prefix of the root textually.
    let escaping = tmp
        .path()
        .join("..")
        .join(outside.path().file_name().unwrap())
        .join("secret.md");
    assert!(escaping.is_file());
    assert!(escaping.starts_with(tmp.path()));

    let bundle = Bundle::load_with_including(
        tmp.path(),
        &LoadOptions::new(),
        std::slice::from_ref(&escaping),
    )
    .unwrap();
    assert_eq!(ids(&bundle), vec!["a"]);
    // The outside file is never read, so it does not even reach the bundle as
    // a path that has no concept id.
    assert!(bundle.parse_errors().is_empty(), "{:?}", bundle.root());
}

#[test]
fn an_included_file_the_walk_already_found_is_not_loaded_twice() {
    let tmp = TempDir::new();
    tmp.write("drafts/a.md", CONCEPT);

    // The same file the walk reports as `<root>/drafts/a.md`, spelled with an
    // interior `.` component.
    let same_file = tmp.path().join("./drafts/a.md");
    let bundle = Bundle::load_with_including(
        tmp.path(),
        &LoadOptions::new(),
        std::slice::from_ref(&same_file),
    )
    .unwrap();
    assert_eq!(ids(&bundle), vec!["drafts/a"]);
}

#[cfg(unix)]
#[test]
fn included_file_behind_a_symlink_out_of_the_root_is_skipped() {
    let tmp = TempDir::new();
    tmp.write("a.md", CONCEPT);
    let outside = TempDir::new();
    outside.write("secret.md", CONCEPT);
    std::os::unix::fs::symlink(outside.path(), tmp.path().join("link")).unwrap();

    // Every component of this path reads as being under the root, and the
    // file it names exists, but it lives outside the bundle.
    let through_link = tmp.path().join("link").join("secret.md");
    assert!(through_link.is_file());
    assert!(through_link.starts_with(tmp.path()));

    let bundle = Bundle::load_with_including(
        tmp.path(),
        &LoadOptions::new(),
        std::slice::from_ref(&through_link),
    )
    .unwrap();
    assert_eq!(ids(&bundle), vec!["a"]);
}

#[test]
fn okfignore_overrides_configured_source() {
    let tmp = TempDir::new();
    tmp.write(".gitignore", "secret.md\n");
    tmp.write(".okfignore", "!secret.md\n");
    tmp.write("secret.md", CONCEPT);

    let opts = LoadOptions::with_ignore(IgnoreConfig::new().with_name(".gitignore"));
    let bundle = Bundle::load_with(tmp.path(), &opts).unwrap();
    assert_eq!(ids(&bundle), vec!["secret"]);
}

#[test]
fn explicit_file_source_missing_is_an_io_error() {
    let tmp = TempDir::new();
    tmp.write("a.md", CONCEPT);
    let opts = LoadOptions::with_ignore(
        IgnoreConfig::new().with_source(IgnoreSource::File(tmp.path().join("nope"))),
    );
    let err = Bundle::load_with(tmp.path(), &opts).expect_err("missing explicit ignore file");
    assert!(matches!(err, okf_core::BundleError::Io { .. }), "{err:?}");
}

#[test]
fn malformed_pattern_does_not_fail_load() {
    let tmp = TempDir::new();
    tmp.write(".okfignore", "[\n");
    tmp.write("a.md", CONCEPT);
    let bundle = Bundle::load(tmp.path()).unwrap();
    assert_eq!(ids(&bundle), vec!["a"]);
}

#[test]
fn two_loads_same_order() {
    let tmp = TempDir::new();
    for n in ["zeta", "alpha", "mid/inner", "mid/other", "beta"] {
        tmp.write(&format!("{n}.md"), CONCEPT);
    }
    let a = ids(&Bundle::load(tmp.path()).unwrap());
    let b = ids(&Bundle::load(tmp.path()).unwrap());
    assert_eq!(a, b);
    let mut sorted = a.clone();
    sorted.sort();
    assert_eq!(a, sorted);
}
