//! Loading and traversing an OKF *bundle*: a directory tree of markdown files
//! (§3).
//!
//! [`Bundle::load`] walks a directory, parses every non-reserved `.md` file
//! into a [`Concept`], records the reserved `index.md` / `log.md` files, and
//! builds two graphs over the result:
//!
//! - the **cross-link graph** from markdown links (§6.1), with backlinks;
//! - the **derivation graph** from `sources[].resource` entries that name
//!   another concept (§5.1), which is how credibility propagates: "when a
//!   `resource` points at another OKF concept, the derivation edge already
//!   exists in the bundle graph, so a consumer MAY recurse into that source's
//!   own `sources`."
//!
//! Loading is **permissive** by design (§11): files whose frontmatter cannot be
//! parsed are collected into [`Bundle::parse_errors`] rather than aborting the
//! load, and broken links are retained as edges to non-existent concepts.

use crate::computation::AttestedComputation;
use crate::concept_id::ConceptId;
use crate::date::{Date, DateTime};
use crate::document::Document;
use crate::error::{BundleError, DocumentError};
use crate::links;
use crate::provenance::Source;
use crate::trust::{Status, TrustTier};
use crate::yaml::Value;
use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

/// Reserved filenames with defined meaning at any level (§3.1).
pub const RESERVED_FILENAMES: [&str; 2] = ["index.md", "log.md"];

/// A single concept within a bundle (one markdown document).
#[derive(Clone, Debug)]
pub struct Concept {
    /// The concept's id (path minus `.md`).
    pub id: ConceptId,
    /// The file path on disk.
    pub path: PathBuf,
    /// The parsed document.
    pub document: Document,
}

impl Concept {
    /// The concept's `type` (§4.1).
    #[must_use]
    pub fn type_(&self) -> Option<Cow<'_, str>> {
        self.document.frontmatter.type_()
    }

    /// The concept's `title`, falling back to the final segment of its id when
    /// none is given, as §4.1 permits.
    #[must_use]
    pub fn display_title(&self) -> String {
        self.document
            .frontmatter
            .title()
            .map_or_else(|| self.id.name().to_string(), std::borrow::Cow::into_owned)
    }

    /// The trust tier derived from `verified` (§5.3).
    #[must_use]
    pub fn trust_tier(&self) -> TrustTier {
        self.document.frontmatter.trust_tier()
    }

    /// The lifecycle `status`; absent means stable (§5.4).
    #[must_use]
    pub fn status(&self) -> Status {
        self.document.frontmatter.status()
    }

    /// Whether this concept is stale at `now`: `now >= stale_after` (§5.5).
    #[must_use]
    pub fn is_stale_at(&self, now: DateTime) -> bool {
        self.document.frontmatter.is_stale_at(now)
    }

    /// Whether `today >= stale_after` (§5.5).
    #[must_use]
    pub fn is_stale_on(&self, today: Date) -> bool {
        self.document.frontmatter.is_stale_on(today)
    }

    /// The `sources` this concept derives from (§5.1).
    #[must_use]
    pub fn sources(&self) -> Vec<Source> {
        self.document.frontmatter.sources()
    }

    /// The Attested Computation contract, when this concept is one (§10).
    #[must_use]
    pub fn attested_computation(&self) -> Option<AttestedComputation> {
        self.document.attested_computation()
    }
}

/// A cross-link from one concept to another, after resolution (§6.1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedLink {
    /// The concept the link points at.
    pub target: ConceptId,
    /// Whether the target concept exists in the bundle. A `false` is allowed:
    /// broken links are not malformed, they may be not-yet-written knowledge.
    pub exists: bool,
    /// The link text.
    pub text: String,
    /// The raw link target as written.
    pub raw: String,
}

/// A `sources` entry resolved against the bundle (§5.1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedSource {
    /// The entry as written in frontmatter.
    pub source: Source,
    /// The concept the entry's `resource` names, when it names one that exists
    /// in this bundle. External URLs and scope descriptors leave this `None`.
    pub concept: Option<ConceptId>,
}

/// A loaded OKF bundle.
#[derive(Debug)]
pub struct Bundle {
    root: PathBuf,
    concepts: Vec<Concept>,
    index: HashMap<ConceptId, usize>,
    index_files: Vec<PathBuf>,
    log_files: Vec<PathBuf>,
    parse_errors: Vec<(PathBuf, DocumentError)>,
    outbound: HashMap<ConceptId, Vec<ResolvedLink>>,
    backlinks: HashMap<ConceptId, Vec<ConceptId>>,
    sources: HashMap<ConceptId, Vec<ResolvedSource>>,
    derived_by: HashMap<ConceptId, Vec<ConceptId>>,
    /// The `okf_version` declared in the bundle-root `index.md` frontmatter
    /// (§12), if any. Cached at load time so [`Bundle::okf_version`] can borrow
    /// instead of re-reading the file on every call.
    okf_version: Option<String>,
}

impl Bundle {
    /// Loads a bundle from a directory tree.
    ///
    /// Returns an error only for I/O failures or a non-directory root. Per-file
    /// parse failures are recorded in [`Bundle::parse_errors`].
    ///
    /// # Errors
    ///
    /// Returns [`BundleError::NotADirectory`] if `root` does not exist or is
    /// not a directory, and [`BundleError::Io`] for any underlying I/O failure
    /// while walking the tree.
    pub fn load(root: impl AsRef<Path>) -> Result<Self, BundleError> {
        let root = root.as_ref().to_path_buf();
        if !root.is_dir() {
            return Err(BundleError::NotADirectory(root));
        }

        let mut md_files = Vec::new();
        collect_markdown(&root, &mut md_files)?;
        md_files.sort();

        // Parse every non-reserved file in parallel. The work per file is
        // I/O-bound (`fs::read_to_string`) followed by CPU-bound
        // (`Document::parse`), so parallelizing across the file list scales
        // with cores on large bundles while staying zero-dependency via
        // `std::thread::scope`. Results are merged in chunk order so the
        // vectors below retain the deterministic sorted order callers rely on.
        let outcomes = parse_files_parallel(&root, &md_files)?;

        let mut concepts = Vec::new();
        let mut index_files = Vec::new();
        let mut log_files = Vec::new();
        let mut parse_errors = Vec::new();
        for outcome in outcomes {
            match outcome {
                FileOutcome::Index(p) => index_files.push(p),
                FileOutcome::Log(p) => log_files.push(p),
                FileOutcome::Concept(c) => concepts.push(c),
                FileOutcome::Error(p, e) => parse_errors.push((p, e)),
            }
        }

        let mut index = HashMap::new();
        for (i, c) in concepts.iter().enumerate() {
            index.insert(c.id.clone(), i);
        }

        let (outbound, backlinks) = build_graph(&concepts, &index);
        let (sources, derived_by) = build_derivation_graph(&concepts, &index);

        // Cache the `okf_version` from the bundle-root `index.md` frontmatter
        // (§12), if any, so `Bundle::okf_version` does not re-read the file on
        // every call.
        let okf_version = read_okf_version(&root);

        Ok(Self {
            root,
            concepts,
            index,
            index_files,
            log_files,
            parse_errors,
            outbound,
            backlinks,
            sources,
            derived_by,
            okf_version,
        })
    }

    /// The bundle's root directory.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// All successfully parsed concepts, in path order.
    #[must_use]
    pub fn concepts(&self) -> &[Concept] {
        &self.concepts
    }

    /// Number of concepts.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.concepts.len()
    }

    /// `true` if the bundle has no concepts.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.concepts.is_empty()
    }

    /// Looks up a concept by id.
    #[must_use]
    pub fn get(&self, id: &ConceptId) -> Option<&Concept> {
        self.index.get(id).map(|&i| &self.concepts[i])
    }

    /// `true` if a concept with this id exists.
    #[must_use]
    pub fn contains(&self, id: &ConceptId) -> bool {
        self.index.contains_key(id)
    }

    /// Paths of all `index.md` files found (§6).
    #[must_use]
    pub fn index_files(&self) -> &[PathBuf] {
        &self.index_files
    }

    /// Paths of all `log.md` files found (§7).
    #[must_use]
    pub fn log_files(&self) -> &[PathBuf] {
        &self.log_files
    }

    /// Files whose frontmatter could not be parsed during loading.
    #[must_use]
    pub fn parse_errors(&self) -> &[(PathBuf, DocumentError)] {
        &self.parse_errors
    }

    /// The resolved outbound cross-links from a concept.
    #[must_use]
    pub fn links_from(&self, id: &ConceptId) -> &[ResolvedLink] {
        self.outbound.get(id).map_or(&[], std::vec::Vec::as_slice)
    }

    /// The ids of concepts that link to the given concept ("cited by" / §
    /// backlinks).
    #[must_use]
    pub fn backlinks(&self, id: &ConceptId) -> &[ConceptId] {
        self.backlinks.get(id).map_or(&[], std::vec::Vec::as_slice)
    }

    /// All broken internal links in the bundle, as `(source, raw_target)`
    /// pairs. Broken links are permitted by the spec (§6.1), so this is
    /// informational.
    #[must_use]
    pub fn broken_links(&self) -> Vec<(ConceptId, String)> {
        let mut out = Vec::new();
        for c in &self.concepts {
            for link in self.links_from(&c.id) {
                if !link.exists {
                    out.push((c.id.clone(), link.raw.clone()));
                }
            }
        }
        out
    }

    /// The declared OKF version from the bundle-root `index.md` frontmatter, if
    /// present (`okf_version`, §12). This is the only place frontmatter is
    /// permitted in an `index.md`.
    ///
    /// Cached at load time, so this is cheap to call repeatedly. A consumer
    /// that does not understand the declared version SHOULD attempt best-effort
    /// consumption rather than refusing the bundle, so this is reported, never
    /// enforced.
    ///
    /// Returns `None` whether the root `index.md` is absent, unreadable, or
    /// lacks the key; a malformed root `index.md` is reported separately by
    /// [`validate_bundle`](crate::validate_bundle).
    #[must_use]
    pub fn okf_version(&self) -> Option<&str> {
        self.okf_version.as_deref()
    }

    /// The concept's `sources` entries, each resolved against the bundle.
    #[must_use]
    pub fn sources_of(&self, id: &ConceptId) -> &[ResolvedSource] {
        self.sources.get(id).map_or(&[], std::vec::Vec::as_slice)
    }

    /// The concepts this one derives from: the `sources[].resource` entries
    /// that name another concept in this bundle (§5.1).
    ///
    /// Following these recursively is how a consumer lets credibility
    /// propagate; external leaf sources carry only their intrinsic signals.
    #[must_use]
    pub fn derived_from(&self, id: &ConceptId) -> Vec<&ConceptId> {
        self.sources_of(id)
            .iter()
            .filter_map(|s| s.concept.as_ref())
            .collect()
    }

    /// The reverse of [`Bundle::derived_from`]: concepts that cite this one as
    /// a source.
    #[must_use]
    pub fn derives(&self, id: &ConceptId) -> &[ConceptId] {
        self.derived_by.get(id).map_or(&[], std::vec::Vec::as_slice)
    }

    /// Every concept whose `type` matches exactly.
    pub fn concepts_of_type<'a>(&'a self, type_: &'a str) -> impl Iterator<Item = &'a Concept> {
        self.concepts
            .iter()
            .filter(move |c| c.type_().as_deref() == Some(type_))
    }

    /// Every `Attested Computation` concept in the bundle (§10.1).
    ///
    /// This is the discovery path §10.5 describes: a consumer reaches a
    /// computation by type, or by following a link from a concept that uses it.
    pub fn attested_computations(&self) -> impl Iterator<Item = &Concept> {
        self.concepts_of_type(crate::computation::ATTESTED_COMPUTATION_TYPE)
    }

    /// A tag index synthesized by scanning frontmatter, tag to concept ids.
    ///
    /// §3.1: OKF does not specify a file format for aggregating documents by
    /// tag, so "a consumer that wants a tag-browsing view can synthesize one at
    /// consumption time." This is that view.
    #[must_use]
    pub fn tags(&self) -> BTreeMap<String, Vec<ConceptId>> {
        let mut out: BTreeMap<String, Vec<ConceptId>> = BTreeMap::new();
        for c in &self.concepts {
            for tag in c.document.frontmatter.tags() {
                out.entry(tag).or_default().push(c.id.clone());
            }
        }
        out
    }

    /// Every concept that is stale at `now`: `now >= stale_after` (§5.5).
    #[must_use]
    pub fn stale_at(&self, now: DateTime) -> Vec<&Concept> {
        self.concepts
            .iter()
            .filter(|c| c.is_stale_at(now))
            .collect()
    }

    /// Every concept that is stale on `today`: `today >= stale_after` (§5.5).
    #[must_use]
    pub fn stale_on(&self, today: Date) -> Vec<&Concept> {
        self.concepts
            .iter()
            .filter(|c| c.is_stale_on(today))
            .collect()
    }

    /// Resolves a path-valued frontmatter field to a file inside the bundle.
    ///
    /// Returns the first candidate from [`links::field_path_candidates`] that
    /// actually exists on disk, or `None` for a URL, a scope descriptor, or a
    /// path that names nothing. Unlike concept links, these fields routinely
    /// point at non-markdown files such as `references/attesters/revenue.py`.
    #[must_use]
    pub fn resolve_path_field(&self, from: &ConceptId, raw: &str) -> Option<PathBuf> {
        links::field_path_candidates(raw, from)
            .into_iter()
            .map(|rel| self.root.join(rel))
            .find(|p| p.is_file())
    }
}

/// The per-file result of loading a single markdown path.
enum FileOutcome {
    Index(PathBuf),
    Log(PathBuf),
    Concept(Concept),
    Error(PathBuf, DocumentError),
}

/// Parses `md_files` in parallel, returning one [`FileOutcome`] per file in the
/// input (sorted) order. I/O failures are fatal and surface as the first
/// [`BundleError`] encountered, matching the sequential loader's `?` semantics.
///
/// Small bundles run inline to avoid thread-spawn overhead; larger ones split
/// the list across one chunk per available core via [`std::thread::scope`].
fn parse_files_parallel(
    root: &Path,
    md_files: &[PathBuf],
) -> Result<Vec<FileOutcome>, BundleError> {
    // Below this threshold, spawning threads costs more than it saves. The
    // number is conservative: parsing a handful of small markdown files takes
    // microseconds.
    const PARALLEL_THRESHOLD: usize = 8;

    if md_files.len() <= PARALLEL_THRESHOLD {
        return md_files
            .iter()
            .map(|p| parse_one(root, p).map_err(BundleError::from))
            .collect();
    }

    let n_threads = std::thread::available_parallelism()
        .map_or(1, usize::from)
        .min(md_files.len());
    // Each thread owns a contiguous slice so the merged output preserves the
    // sorted input order without a re-sort.
    let chunk_size = md_files.len().div_ceil(n_threads);
    let chunks: Vec<&[PathBuf]> = md_files.chunks(chunk_size).collect();

    let results = std::thread::scope(|scope| {
        chunks
            .iter()
            .map(|chunk| scope.spawn(|| parse_chunk(root, chunk)))
            .map(|h| h.join().expect("worker thread panicked"))
            .collect::<Vec<Result<Vec<FileOutcome>, BundleError>>>()
    });

    // Surface the first I/O error in chunk order, matching the sequential
    // loader's behavior of failing on the earliest error in sorted file order.
    let mut merged = Vec::with_capacity(md_files.len());
    for result in results {
        for outcome in result? {
            merged.push(outcome);
        }
    }
    Ok(merged)
}

/// Parses one chunk of files on a single thread.
fn parse_chunk(root: &Path, chunk: &[PathBuf]) -> Result<Vec<FileOutcome>, BundleError> {
    chunk
        .iter()
        .map(|p| parse_one(root, p).map_err(BundleError::from))
        .collect()
}

/// Loads and classifies a single markdown file. `fs::read_to_string` failures
/// propagate as [`BundleError::Io`]; frontmatter and concept-id failures are
/// collected as [`FileOutcome::Error`] for the permissive-load path (§11).
fn parse_one(root: &Path, path: &Path) -> Result<FileOutcome, std::io::Error> {
    let filename = path
        .file_name()
        .map(|f| f.to_string_lossy().into_owned())
        .unwrap_or_default();
    match filename.as_str() {
        "index.md" => Ok(FileOutcome::Index(path.to_path_buf())),
        "log.md" => Ok(FileOutcome::Log(path.to_path_buf())),
        _ => {
            let text = fs::read_to_string(path)?;
            let outcome = match Document::parse(&text) {
                Ok(document) => match ConceptId::from_path(root, path) {
                    Ok(id) => FileOutcome::Concept(Concept {
                        id,
                        path: path.to_path_buf(),
                        document,
                    }),
                    Err(e) => FileOutcome::Error(path.to_path_buf(), e.into()),
                },
                Err(e) => FileOutcome::Error(path.to_path_buf(), e),
            };
            Ok(outcome)
        }
    }
}

/// Reads `okf_version` from the bundle-root `index.md` frontmatter (§12), if
/// the file exists and the key is present as a string scalar. Returns `None`
/// for a missing file, an unparseable `index.md`, or a non-string value.
fn read_okf_version(root: &Path) -> Option<String> {
    let text = fs::read_to_string(root.join("index.md")).ok()?;
    let doc = Document::parse(&text).ok()?;
    doc.frontmatter
        .get("okf_version")
        .and_then(Value::as_str)
        .map(str::to_owned)
}

/// Recursively collects `*.md` file paths under `dir`.
fn collect_markdown(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), BundleError> {
    let mut entries: Vec<_> = fs::read_dir(dir)?.collect::<Result<_, _>>()?;
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for entry in entries {
        let path = entry.path();
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            collect_markdown(&path, out)?;
        } else if file_type.is_file() && path.extension().is_some_and(|e| e == "md") {
            out.push(path);
        }
    }
    Ok(())
}

/// Builds the outbound link and backlink maps for all concepts.
fn build_graph(
    concepts: &[Concept],
    index: &HashMap<ConceptId, usize>,
) -> (
    HashMap<ConceptId, Vec<ResolvedLink>>,
    HashMap<ConceptId, Vec<ConceptId>>,
) {
    let mut outbound: HashMap<ConceptId, Vec<ResolvedLink>> = HashMap::new();
    let mut backlinks: HashMap<ConceptId, Vec<ConceptId>> = HashMap::new();

    for c in concepts {
        let mut resolved = Vec::new();
        for link in c.document.links() {
            // A percent-encoded target has two readings (§6.1); take whichever
            // names a concept that is really there, else the literal one so the
            // link is still reported as broken rather than dropped.
            let candidates = link.resolve_all(&c.id);
            let target = candidates
                .iter()
                .find(|t| index.contains_key(*t))
                .or_else(|| candidates.first())
                .cloned();
            if let Some(target) = target {
                let exists = index.contains_key(&target);
                if exists {
                    let entry = backlinks.entry(target.clone()).or_default();
                    if !entry.contains(&c.id) {
                        entry.push(c.id.clone());
                    }
                }
                resolved.push(ResolvedLink {
                    target,
                    exists,
                    text: link.text,
                    raw: link.target,
                });
            }
        }
        outbound.insert(c.id.clone(), resolved);
    }

    (outbound, backlinks)
}

/// Builds the derivation graph: every concept's `sources` entries, with the
/// ones naming another concept in the bundle resolved to its id (§5.1).
fn build_derivation_graph(
    concepts: &[Concept],
    index: &HashMap<ConceptId, usize>,
) -> (
    HashMap<ConceptId, Vec<ResolvedSource>>,
    HashMap<ConceptId, Vec<ConceptId>>,
) {
    let mut sources: HashMap<ConceptId, Vec<ResolvedSource>> = HashMap::new();
    let mut derived_by: HashMap<ConceptId, Vec<ConceptId>> = HashMap::new();

    for c in concepts {
        let entries: Vec<ResolvedSource> = c
            .sources()
            .into_iter()
            .map(|source| {
                let concept = source
                    .resource
                    .as_deref()
                    .and_then(|raw| resolve_concept_reference(index, &c.id, raw))
                    .filter(|target| target != &c.id);
                if let Some(target) = &concept {
                    let entry = derived_by.entry(target.clone()).or_default();
                    if !entry.contains(&c.id) {
                        entry.push(c.id.clone());
                    }
                }
                ResolvedSource { source, concept }
            })
            .collect();
        if !entries.is_empty() {
            sources.insert(c.id.clone(), entries);
        }
    }

    (sources, derived_by)
}

/// Resolves a raw path-valued reference to a concept that exists in the bundle.
///
/// Both spellings are tried, with and without the `.md` suffix, and both
/// readings of a relative path (§6.2). Requiring the target to exist keeps
/// scope descriptors and external URLs from being mistaken for concepts.
fn resolve_concept_reference(
    index: &HashMap<ConceptId, usize>,
    from: &ConceptId,
    raw: &str,
) -> Option<ConceptId> {
    for candidate in links::field_path_candidates(raw, from) {
        let ids = [
            links::concept_id_for_path(&candidate),
            ConceptId::parse(&candidate).ok(),
        ];
        for id in ids.into_iter().flatten() {
            if index.contains_key(&id) {
                return Some(id);
            }
        }
    }
    None
}
