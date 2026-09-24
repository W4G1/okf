//! The one directory walker every bundle operation uses.
//!
//! Loading a bundle, regenerating indexes, remediating, formatting, and the
//! studio watcher all need "the markdown files in this bundle". They used to
//! each answer that differently; this module answers it once, honouring the
//! ignore rules in [`crate::ignore`]:
//!
//! 1. the default rules ([`crate::ignore::DEFAULT_IGNORE_PATTERNS`]: hidden
//!    directories, `target`, `node_modules`), lowest precedence;
//! 2. each [`IgnoreSource`] in the caller's [`IgnoreConfig`], in order;
//! 3. `.okfignore`, always.
//!
//! Ignore-file *names* are read from every directory entered and from the
//! bundle root's ancestors up to the nearest directory containing `.git`.
//! Deeper files override shallower ones; within one directory, later sources
//! override earlier ones. An explicit [`IgnoreSource::File`] is read once and
//! applied once, at its own directory, in its command-line position among the
//! sources found there: command-line order decides between it and a name
//! discovered alongside it, while that directory's `.okfignore` and anything
//! deeper still outrank it. A `.dockerignore`, named or explicit, applies at
//! the bundle root only. Ignored directories are pruned, never entered.
//!
//! An `index.md` is never ignored on its own: every bundle directory has
//! one, so a rule that matches it is overridden, and [`ignored_index_files`]
//! lists the paths where that happens so a caller can warn. An `index.md`
//! inside an ignored directory is still skipped with the rest of it.

use crate::ignore::{Flavor, IgnoreConfig, IgnoreMatcher, IgnoreRules, IgnoreSource, OKFIGNORE};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

const INDEX_FILE: &str = "index.md";

/// Options for one walk.
#[derive(Clone, Copy, Debug)]
pub struct WalkOptions<'a> {
    /// Extra ignore sources on top of the built-in rules and `.okfignore`.
    pub ignore: &'a IgnoreConfig,
}

impl<'a> WalkOptions<'a> {
    /// Options with the given ignore configuration.
    #[must_use]
    pub const fn new(ignore: &'a IgnoreConfig) -> Self {
        Self { ignore }
    }
}

/// Every `*.md` file under `root` that is not ignored, sorted by path.
///
/// # Errors
///
/// Returns the underlying [`io::Error`] for a directory that cannot be read
/// or an explicit [`IgnoreSource::File`] that cannot be read.
pub fn walk_markdown(root: &Path, opts: &WalkOptions<'_>) -> io::Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    walk_entries(root, opts, &mut |path, ft| {
        if ft.is_file() && path.extension().is_some_and(|e| e == "md") {
            out.push(path.to_path_buf());
        }
    })?;
    out.sort();
    Ok(out)
}

/// Visits every non-ignored entry under `root`, depth-first, in `file_name`
/// order within each directory. Directories are reported before their
/// contents. `root` itself is not reported.
///
/// Paths passed to `f` are `root` joined with the relative path, so they take
/// whatever form (relative or absolute) `root` has.
///
/// # Errors
///
/// Returns the underlying [`io::Error`] for a directory that cannot be read
/// or an explicit [`IgnoreSource::File`] that cannot be read.
pub fn walk_entries(
    root: &Path,
    opts: &WalkOptions<'_>,
    f: &mut dyn FnMut(&Path, &fs::FileType),
) -> io::Result<()> {
    walk(
        root,
        *opts,
        &mut Visit {
            f,
            index_rule: None,
            rule_file: None,
        },
    )
}

/// Visits every non-ignored entry like [`walk_entries`], and additionally
/// reports every ignore file the walk reads.
///
/// Each is reported by absolute path: the explicit [`IgnoreSource::File`]s,
/// and each discovered file in the root's ancestors and in every directory
/// entered. Those files decide which entries the walk reports, so a caller that
/// fingerprints the walked files to detect changes must fingerprint these
/// too. A rule file is reported even when the rules match it, since the walk
/// still reads it.
///
/// # Errors
///
/// As [`walk_entries`].
pub fn walk_entries_with_rule_files(
    root: &Path,
    opts: &WalkOptions<'_>,
    f: &mut dyn FnMut(&Path, &fs::FileType),
    rule_file: &mut dyn FnMut(&Path),
) -> io::Result<()> {
    walk(
        root,
        *opts,
        &mut Visit {
            f,
            index_rule: None,
            rule_file: Some(rule_file),
        },
    )
}

/// The `index.md` paths under `root` that an ignore rule matches, sorted.
///
/// The walk keeps those files anyway, so each rule listed here has no
/// effect. A path is listed whether or not the file exists yet, since index
/// regeneration will create it. Directories the rules exclude are not
/// entered, so their index files are neither kept nor listed.
///
/// # Errors
///
/// As [`walk_entries`].
pub fn ignored_index_files(root: &Path, opts: &WalkOptions<'_>) -> io::Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    walk(
        root,
        *opts,
        &mut Visit {
            f: &mut |_, _| {},
            index_rule: Some(&mut |path| out.push(path.to_path_buf())),
            rule_file: None,
        },
    )?;
    out.sort();
    Ok(out)
}

/// What one walk reports: every surviving entry, optionally each `index.md`
/// a rule matches, and optionally every ignore file read.
struct Visit<'f> {
    f: &'f mut dyn FnMut(&Path, &fs::FileType),
    index_rule: Option<Report<'f>>,
    rule_file: Option<Report<'f>>,
}

/// Called with one path: an `index.md` a rule matches, or an ignore file the
/// walk read.
type Report<'f> = &'f mut dyn FnMut(&Path);

fn walk(root: &Path, opts: WalkOptions<'_>, visit: &mut Visit<'_>) -> io::Result<()> {
    let abs_root = absolute(root)?;
    // Both the seed and the per-source anchoring need the ancestors, so they
    // are resolved once for the whole walk.
    let ancestors = ancestor_dirs(&abs_root);
    let layers = Layers::read(opts.ignore, &abs_root, &ancestors)?;
    if let Some(report) = visit.rule_file.as_mut() {
        for source in &opts.ignore.sources {
            if let IgnoreSource::File(path) = source {
                report(&absolute(path)?);
            }
        }
    }
    let mut matcher = seed_matcher(&abs_root, &ancestors, &layers, &mut visit.rule_file);
    walk_dir(root, &abs_root, &abs_root, &layers, &mut matcher, visit)
}

/// The ignore configuration together with its explicit files, read once for
/// the whole walk. Each entry of `files` lines up with the source at the same
/// position in [`IgnoreConfig::sources`], paired with the directory its layer
/// belongs at, which is what keeps its configured precedence.
struct Layers<'a> {
    config: &'a IgnoreConfig,
    files: Vec<Option<(PathBuf, IgnoreRules)>>,
}

impl<'a> Layers<'a> {
    /// Reads every [`IgnoreSource::File`] in `config` and decides where each
    /// one's layer belongs.
    ///
    /// # Errors
    ///
    /// Returns the underlying [`io::Error`] for a file that cannot be read.
    fn read(config: &'a IgnoreConfig, abs_root: &Path, ancestors: &[PathBuf]) -> io::Result<Self> {
        let mut files = Vec::with_capacity(config.sources.len());
        for source in &config.sources {
            match source {
                IgnoreSource::Name(_) => files.push(None),
                IgnoreSource::File(path) => {
                    let rules = IgnoreRules::read(&absolute(path)?, source.flavor())?;
                    // An explicit file behaves like an ignore file living in
                    // its own directory, so its layer belongs at that level
                    // and is pushed once. A base the walk never enters (a
                    // sibling tree, or an ancestor past the repository
                    // boundary) is applied at the root, the shallowest level
                    // the walk does visit. So is every Docker file, since
                    // the root is the only level a Docker layer is pushed.
                    let base = rules.base().to_path_buf();
                    let anchor = if source.flavor() != Flavor::Docker
                        && (base.starts_with(abs_root) || ancestors.contains(&base))
                    {
                        base
                    } else {
                        abs_root.to_path_buf()
                    };
                    files.push(Some((anchor, rules)));
                }
            }
        }
        Ok(Self { config, files })
    }
}

/// Builds the matcher every walk starts from: the default rules, then the
/// ignore layers found in the root's ancestors. Everything pushed after the
/// defaults outranks them, so any ignore file can re-include a
/// default-skipped directory.
fn seed_matcher(
    abs_root: &Path,
    ancestors: &[PathBuf],
    layers: &Layers<'_>,
    rule_file: &mut Option<Report<'_>>,
) -> IgnoreMatcher {
    let mut matcher = IgnoreMatcher::new();
    matcher.push(IgnoreRules::builtin(abs_root));

    // Ancestors, shallowest first, so deeper ones are pushed later and win.
    for dir in ancestors.iter().rev() {
        push_dir_layers(&mut matcher, dir, abs_root, layers, rule_file);
    }
    matcher
}

/// An absolute path with `.` and `..` resolved lexically.
///
/// [`std::path::absolute`] keeps `..` components; a rule base of
/// `/repo/bundle/../shared.ignore` would then never prefix-match the
/// `/repo/bundle/...` entries it is meant to govern. Lexical resolution is
/// deliberate: `fs::canonicalize` would also follow symlinks and could put
/// the root and an ignore file on different sides of one (macOS's `/var` vs
/// `/private/var`), which breaks prefix matching the same way.
fn absolute(path: &Path) -> io::Result<PathBuf> {
    use std::path::Component;
    let abs = std::path::absolute(path)?;
    let mut out = PathBuf::new();
    for comp in abs.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    Ok(out)
}

/// `path` re-expressed under `root`, or `None` when it does not lie under it.
///
/// Containment is decided on the canonical paths. [`Path::starts_with`] alone
/// is textual, so it accepts `root/../outside/a.md`; and a symlinked
/// directory inside the bundle can name a file outside it while every
/// component still reads as being under the root. A walk never follows a
/// symlink out of the tree either, so canonicalizing is what makes the two
/// agree. Resolving both sides the same way is what makes [`fs::canonicalize`]
/// right here, unlike in [`absolute`], where it would compare a resolved rule
/// base against unresolved walked paths.
///
/// The returned path keeps `root`'s own spelling followed by the canonical
/// remainder, which is the form [`walk_entries`] reports. That makes the
/// result comparable to a walked path however the caller spelled its input,
/// and strippable back to a bundle-relative path. A caller naming one file in
/// a bundle wants this before deriving a [`crate::ConceptId`] from it, so that
/// the id matches the one the loaded bundle holds.
///
/// # Errors
///
/// Returns the underlying [`io::Error`] if either path cannot be resolved.
pub fn under_root(root: &Path, path: &Path) -> io::Result<Option<PathBuf>> {
    let canonical_root = fs::canonicalize(root)?;
    Ok(fs::canonicalize(path)?
        .strip_prefix(&canonical_root)
        .ok()
        .map(|rel| root.join(rel)))
}

/// The bundle root's ancestors up to and including the nearest one that
/// contains `.git`, nearest first. Empty when the root itself is a repository
/// root or no ancestor is.
fn ancestor_dirs(abs_root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if abs_root.join(".git").exists() {
        return out;
    }
    let mut cur = abs_root.parent();
    while let Some(dir) = cur {
        out.push(dir.to_path_buf());
        if dir.join(".git").exists() {
            return out;
        }
        cur = dir.parent();
    }
    // Reached the filesystem root without finding a repository.
    Vec::new()
}

/// Pushes `dir/name` as a layer if it exists and is readable. An existing
/// file is reported to `rule_file` even when unreadable, so a later
/// permission fix is still seen as a change.
fn push_if_present(
    matcher: &mut IgnoreMatcher,
    dir: &Path,
    name: &str,
    flavor: Flavor,
    rule_file: &mut Option<Report<'_>>,
) -> bool {
    let path = dir.join(name);
    if !path.is_file() {
        return false;
    }
    if let Some(report) = rule_file.as_mut() {
        report(&path);
    }
    IgnoreRules::read(&path, flavor).is_ok_and(|rules| {
        matcher.push(rules);
        true
    })
}

/// Pushes this directory's ignore layers in precedence order -- every
/// configured source in its command-line position, then `.okfignore` -- and
/// returns how many layers were pushed so the caller can pop them.
fn push_dir_layers(
    matcher: &mut IgnoreMatcher,
    abs_dir: &Path,
    abs_root: &Path,
    layers: &Layers<'_>,
    rule_file: &mut Option<Report<'_>>,
) -> usize {
    let mut pushed = 0;
    for (source, file) in layers.config.sources.iter().zip(&layers.files) {
        let flavor = source.flavor();
        // A Docker ignore file applies at the bundle root only, whether it was
        // discovered by name or named as an explicit path. Its rules are all
        // root-anchored, so re-applying it deeper would only let it outrank a
        // nested source that re-includes one of the paths it matches.
        if flavor == Flavor::Docker && abs_dir != abs_root {
            continue;
        }
        match source {
            IgnoreSource::Name(name) => {
                if push_if_present(matcher, abs_dir, name, flavor, rule_file) {
                    pushed += 1;
                }
            }
            // An explicit file is read once and pushed once, at its own
            // directory, in its command-line position among the sources
            // found there. Re-applying it deeper would let it outrank a
            // shallower `.okfignore`, which always has the last word.
            IgnoreSource::File(_) => {
                if let Some((anchor, rules)) = file
                    && anchor == abs_dir
                {
                    matcher.push(rules.clone());
                    pushed += 1;
                }
            }
        }
    }
    if push_if_present(matcher, abs_dir, OKFIGNORE, Flavor::Gitignore, rule_file) {
        pushed += 1;
    }
    pushed
}

fn walk_dir(
    dir: &Path,
    abs_dir: &Path,
    abs_root: &Path,
    layers: &Layers<'_>,
    matcher: &mut IgnoreMatcher,
    visit: &mut Visit<'_>,
) -> io::Result<()> {
    let pushed = push_dir_layers(matcher, abs_dir, abs_root, layers, &mut visit.rule_file);

    // Checked here rather than per entry, so an index file that does not
    // exist yet is reported too.
    if let Some(report) = visit.index_rule.as_mut()
        && matcher.is_ignored(&abs_dir.join(INDEX_FILE), false)
    {
        report(&dir.join(INDEX_FILE));
    }

    let mut entries: Vec<fs::DirEntry> = fs::read_dir(dir)?.collect::<Result<_, _>>()?;
    entries.sort_by_key(fs::DirEntry::file_name);

    let result = (|| {
        for entry in entries {
            let name = entry.file_name();
            let path = dir.join(&name);
            let abs_path = abs_dir.join(&name);
            let file_type = entry.file_type()?;
            let is_dir = file_type.is_dir();
            let is_index = !is_dir && name == INDEX_FILE;
            if !is_index && matcher.is_ignored(&abs_path, is_dir) {
                continue;
            }
            (visit.f)(&path, &file_type);
            if is_dir {
                walk_dir(&path, &abs_path, abs_root, layers, matcher, visit)?;
            }
        }
        Ok(())
    })();

    for _ in 0..pushed {
        matcher.pop();
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

    struct Tmp(PathBuf);
    impl Tmp {
        fn new() -> Self {
            let n = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let c = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let p = std::env::temp_dir().join(format!("okf-walk-{}-{n}-{c}", std::process::id()));
            fs::create_dir_all(&p).unwrap();
            Self(p)
        }
        fn write(&self, rel: &str, text: &str) {
            let p = self.0.join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, text).unwrap();
        }
        fn md(&self, rel: &str) {
            self.write(rel, "---\ntype: X\n---\n");
        }
    }
    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn rels(root: &Path, files: &[PathBuf]) -> Vec<String> {
        files
            .iter()
            .map(|p| {
                p.strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect()
    }

    fn walk(root: &Path, cfg: &IgnoreConfig) -> Vec<String> {
        rels(root, &walk_markdown(root, &WalkOptions::new(cfg)).unwrap())
    }

    #[test]
    fn sorted_markdown_and_default_skips() {
        let t = Tmp::new();
        t.md("b.md");
        t.md("a.md");
        t.md("sub/c.md");
        t.md(".git/notes.md");
        t.md(".drafts/d.md");
        t.md("node_modules/pkg/README.md");
        t.md("sub/target/out.md");
        t.md(".hidden-file.md");
        t.write("not-md.txt", "x");
        let got = walk(&t.0, &IgnoreConfig::new());
        // Hidden directories, `target`, and `node_modules` are skipped at
        // any depth; a hidden *file* is not a directory and is kept.
        assert_eq!(got, vec![".hidden-file.md", "a.md", "b.md", "sub/c.md"]);
    }

    #[test]
    fn okfignore_always_applies_and_nests() {
        let t = Tmp::new();
        t.write(".okfignore", "*.wip.md\nscratch/\n");
        t.write("team/.okfignore", "!*.wip.md\n");
        t.md("a.wip.md");
        t.md("team/a.wip.md");
        t.md("scratch/x.md");
        t.md("keep.md");
        let got = walk(&t.0, &IgnoreConfig::new());
        assert_eq!(got, vec!["keep.md", "team/a.wip.md"]);
    }

    #[test]
    fn source_precedence_within_directory() {
        let t = Tmp::new();
        t.write(".a-ignore", "x.md\ny.md\n");
        t.write(".b-ignore", "!x.md\n");
        t.write(".okfignore", "!y.md\nz.md\n");
        t.md("x.md");
        t.md("y.md");
        t.md("z.md");
        let cfg = IgnoreConfig::new()
            .with_name(".a-ignore")
            .with_name(".b-ignore");
        let got = walk(&t.0, &cfg);
        assert_eq!(got, vec!["x.md", "y.md"]);
    }

    #[test]
    fn configured_order_decides_between_file_and_name() {
        let t = Tmp::new();
        t.write("shared.ignore", "a.md\n");
        t.write(".gitignore", "!a.md\n");
        t.md("a.md");
        t.md("b.md");
        let file = IgnoreSource::File(t.0.join("shared.ignore"));

        let name_last = IgnoreConfig::new()
            .with_source(file.clone())
            .with_name(".gitignore");
        assert_eq!(walk(&t.0, &name_last), vec!["a.md", "b.md"]);

        let file_last = IgnoreConfig::new()
            .with_name(".gitignore")
            .with_source(file);
        assert_eq!(walk(&t.0, &file_last), vec!["b.md"]);
    }

    #[test]
    fn okfignore_outranks_an_explicit_file_source() {
        let t = Tmp::new();
        t.write("shared.ignore", "!keep.md\nb.md\n");
        t.write(".okfignore", "keep.md\n!b.md\n");
        t.md("keep.md");
        t.md("b.md");
        let cfg = IgnoreConfig::new().with_source(IgnoreSource::File(t.0.join("shared.ignore")));
        assert_eq!(walk(&t.0, &cfg), vec!["b.md"]);
    }

    #[test]
    fn index_files_survive_rules_that_match_them() {
        let t = Tmp::new();
        t.write(".okfignore", "*.md\n!keep.md\ndrafts/\n");
        t.md("index.md");
        t.md("keep.md");
        t.md("other.md");
        t.md("sub/index.md");
        t.md("drafts/index.md");
        let cfg = IgnoreConfig::new();
        // An ignored directory still takes its index file with it.
        assert_eq!(
            walk(&t.0, &cfg),
            vec!["index.md", "keep.md", "sub/index.md"]
        );
    }

    /// Listed whether or not the file exists, since regeneration creates it.
    #[test]
    fn ignored_index_files_lists_every_matched_index_path() {
        let t = Tmp::new();
        t.write(".okfignore", "/index.md\ndrafts/\n");
        t.write("deep/.okfignore", "index.md\n");
        t.md("a.md");
        t.md("deep/b.md");
        t.md("plain/c.md");
        t.md("drafts/d.md");

        let cfg = IgnoreConfig::new();
        let got = ignored_index_files(&t.0, &WalkOptions::new(&cfg)).unwrap();
        // The root rule is anchored, so it governs the root only; `deep/`
        // has its own rule, `plain/` has none, and `drafts/` is not entered.
        assert_eq!(rels(&t.0, &got), vec!["deep/index.md", "index.md"]);
    }

    #[test]
    fn an_explicit_file_does_not_outrank_a_shallower_okfignore() {
        let t = Tmp::new();
        t.write("shared.ignore", "secret.md\n");
        t.write(".okfignore", "!secret.md\n");
        t.md("secret.md");
        t.md("sub/secret.md");
        // The root `.okfignore` re-includes the file, and the explicit source
        // sits at that same level below it, so the negation holds at every
        // depth rather than being overridden inside `sub/`.
        let cfg = IgnoreConfig::new().with_source(IgnoreSource::File(t.0.join("shared.ignore")));
        assert_eq!(walk(&t.0, &cfg), vec!["secret.md", "sub/secret.md"]);
    }

    #[test]
    fn an_explicit_dockerignore_applies_at_the_root_only() {
        let t = Tmp::new();
        t.write(".dockerignore", "sub/keep.md\n");
        t.write("sub/.gitignore", "!keep.md\n");
        t.md("sub/keep.md");
        t.md("sub/other.md");
        // Like a discovered `.dockerignore`, an explicit one is applied only
        // at the root, so the nested `.gitignore` is the deeper layer and its
        // negation decides.
        let cfg = IgnoreConfig::new()
            .with_name(".gitignore")
            .with_source(IgnoreSource::File(t.0.join(".dockerignore")));
        assert_eq!(walk(&t.0, &cfg), vec!["sub/keep.md", "sub/other.md"]);
    }

    #[test]
    fn an_explicit_dockerignore_outside_the_root_still_applies() {
        let t = Tmp::new();
        // The repository makes the file's directory an ancestor the walk
        // visits, which is not where a Docker layer may be pushed.
        fs::create_dir_all(t.0.join(".git")).unwrap();
        t.write(".dockerignore", "bundle/drafts\n");
        t.md("bundle/drafts/a.md");
        t.md("bundle/b.md");
        let root = t.0.join("bundle");
        let cfg = IgnoreConfig::new().with_source(IgnoreSource::File(t.0.join(".dockerignore")));
        assert_eq!(walk(&root, &cfg), vec!["b.md"]);
    }

    #[test]
    fn default_skips_apply_everywhere_and_are_overridable() {
        let t = Tmp::new();
        t.md(".notes/n.md");
        t.md("node_modules/pkg/README.md");
        t.md("target/out.md");
        t.md("a.md");
        assert_eq!(walk(&t.0, &IgnoreConfig::new()), vec!["a.md"]);

        // A named source and .okfignore each re-include one directory.
        t.write(".x-ignore", "!target/\n");
        t.write(".okfignore", "!.notes/\n");
        let cfg = IgnoreConfig::new().with_name(".x-ignore");
        assert_eq!(
            walk(&t.0, &cfg),
            vec![".notes/n.md", "a.md", "target/out.md"]
        );

        // An ancestor ignore file can too.
        let t2 = Tmp::new();
        fs::create_dir_all(t2.0.join(".git")).unwrap();
        t2.write(".gitignore", "!node_modules/\n");
        t2.md("kb/node_modules/pkg/README.md");
        t2.md("kb/a.md");
        let root = t2.0.join("kb");
        assert_eq!(
            walk(&root, &IgnoreConfig::new().with_name(".gitignore")),
            vec!["a.md", "node_modules/pkg/README.md"]
        );
    }

    #[test]
    fn ancestor_gitignore_applies_up_to_repo_root() {
        let t = Tmp::new();
        fs::create_dir_all(t.0.join(".git")).unwrap();
        t.write(".gitignore", "*.tmp.md\ndocs/kb/private/\n");
        t.write("docs/.gitignore", "mid.md\n");
        t.md("docs/kb/x.tmp.md");
        t.md("docs/kb/private/a.md");
        t.md("docs/kb/mid.md");
        t.md("docs/kb/ok.md");
        let root = t.0.join("docs/kb");
        let got = walk(&root, &IgnoreConfig::new().with_name(".gitignore"));
        assert_eq!(got, vec!["ok.md"]);
    }

    #[test]
    fn no_repo_means_no_ancestor_search() {
        let t = Tmp::new();
        t.write(".gitignore", "*.md\n");
        t.md("bundle/a.md");
        let root = t.0.join("bundle");
        let got = walk(&root, &IgnoreConfig::new().with_name(".gitignore"));
        assert_eq!(got, vec!["a.md"]);
    }

    #[test]
    fn root_that_is_repo_root_does_not_search_ancestors() {
        let t = Tmp::new();
        t.write(".gitignore", "*.md\n");
        fs::create_dir_all(t.0.join("bundle/.git")).unwrap();
        t.md("bundle/a.md");
        let root = t.0.join("bundle");
        let got = walk(&root, &IgnoreConfig::new().with_name(".gitignore"));
        assert_eq!(got, vec!["a.md"]);
    }

    #[test]
    fn ignored_directory_is_pruned_and_not_read() {
        let t = Tmp::new();
        t.write(".okfignore", "build\n!build/keep.md\n");
        t.md("build/keep.md");
        t.md("a.md");
        let mut visited = Vec::new();
        walk_entries(
            &t.0,
            &WalkOptions::new(&IgnoreConfig::new()),
            &mut |p, _| {
                visited.push(p.to_path_buf());
            },
        )
        .unwrap();
        let names = rels(&t.0, &visited);
        assert!(!names.iter().any(|n| n.starts_with("build")), "{names:?}");
        assert_eq!(names, vec![".okfignore", "a.md"]);
    }

    #[test]
    fn dockerignore_is_root_only_and_anchored() {
        let t = Tmp::new();
        t.write(".dockerignore", "README.md\n");
        t.write("sub/.dockerignore", "*.md\n");
        t.md("README.md");
        t.md("sub/README.md");
        t.md("sub/a.md");
        let got = walk(&t.0, &IgnoreConfig::new().with_name(".dockerignore"));
        assert_eq!(got, vec!["sub/README.md", "sub/a.md"]);
    }

    #[test]
    fn explicit_file_source_is_anchored_to_its_parent() {
        let t = Tmp::new();
        t.write("shared.ignore", "bundle/private/\n");
        t.md("bundle/private/a.md");
        t.md("bundle/b.md");
        let root = t.0.join("bundle");
        let cfg = IgnoreConfig::new().with_source(IgnoreSource::File(t.0.join("shared.ignore")));
        let got = walk(&root, &cfg);
        assert_eq!(got, vec!["b.md"]);
    }

    #[test]
    fn missing_explicit_file_is_an_error() {
        let t = Tmp::new();
        t.md("a.md");
        let cfg = IgnoreConfig::new().with_source(IgnoreSource::File(t.0.join("nope")));
        let err = walk_markdown(&t.0, &WalkOptions::new(&cfg)).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
    }

    #[test]
    fn missing_named_source_is_silent() {
        let t = Tmp::new();
        t.md("a.md");
        let got = walk(&t.0, &IgnoreConfig::new().with_name(".noignore"));
        assert_eq!(got, vec!["a.md"]);
    }

    #[test]
    fn relative_root_paths_stay_relative() {
        let t = Tmp::new();
        t.write(".okfignore", "skip.md\n");
        t.md("skip.md");
        t.md("a.md");
        let cwd = std::env::current_dir().unwrap();
        // Use a relative root by walking a path expressed relative to cwd.
        let rel = pathdiff(&t.0, &cwd);
        let files = walk_markdown(&rel, &WalkOptions::new(&IgnoreConfig::new())).unwrap();
        assert_eq!(files.len(), 1);
        assert!(files[0].starts_with(&rel), "{files:?} vs {rel:?}");
        assert!(files[0].ends_with("a.md"));
    }

    /// The spec's non-UTF-8 scenario at walk level: the file must survive the
    /// walk and be treated as not ignored even by a catch-all pattern.
    #[cfg(unix)]
    #[test]
    fn non_utf8_filename_survives_walk_and_is_not_ignored() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;
        let t = Tmp::new();
        t.write(".okfignore", "*\n");
        t.md("plain.md");
        let bad_name = OsStr::from_bytes(b"\xff\xfe.md");
        if fs::write(t.0.join(bad_name), "---\ntype: X\n---\n").is_err() {
            // APFS and some other filesystems reject invalid UTF-8 names
            // outright (EILSEQ); the scenario is exercised on Linux CI.
            eprintln!("skipping: filesystem rejects non-UTF-8 filenames");
            return;
        }

        let files = walk_markdown(&t.0, &WalkOptions::new(&IgnoreConfig::new()))
            .expect("walk must not fail on a non-UTF-8 name");
        assert_eq!(files.len(), 1, "{files:?}");
        assert_eq!(files[0].file_name(), Some(bad_name));
    }

    /// Minimal relative-path computation for the test above.
    fn pathdiff(target: &Path, base: &Path) -> PathBuf {
        let t: Vec<_> = target.components().collect();
        let b: Vec<_> = base.components().collect();
        let common = t.iter().zip(&b).take_while(|(x, y)| x == y).count();
        let mut out = PathBuf::new();
        for _ in common..b.len() {
            out.push("..");
        }
        for c in &t[common..] {
            out.push(c);
        }
        out
    }
}
