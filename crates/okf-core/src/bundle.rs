//! Loading and traversing an OKF *bundle*: a directory tree of markdown files.
//!
//! [`Bundle::load`] walks a directory, decides which files are concepts,
//! records the reserved `index.md` / `log.md` files, and builds the bundle
//! graph over the result.
//!
//! ## Which files are concepts
//!
//! 1. A `.md` file that is not reserved and has frontmatter with a non-empty
//!    `type` is a [`Concept`].
//! 2. A `.md` file with no frontmatter, or no `type`, is not a concept. It is
//!    skipped and listed in [`Bundle::non_concepts`]; it does not affect
//!    conformance. This is what lets a bundle hold a `README.md`.
//! 3. A file whose name does not end in `.md` is an asset. It may be the
//!    target of a link or a path field, but it is not a concept.
//! 4. Files and directories whose names start with `.` are not part of the
//!    bundle.
//! 5. Only files under the bundle root are read. A symbolic link that leads
//!    out of the root is not followed.
//!
//! ## The bundle graph
//!
//! The graph has two kinds of directed edges, both resolved with the same
//! rules (see [`links`]):
//!
//! - a **link** from a concept to a concept ([`Bundle::links_from`], with
//!   [`Bundle::backlinks`] for the reverse);
//! - a **path field** that resolves to a concept ([`Bundle::path_edges_from`],
//!   with [`Bundle::referenced_by`] for the reverse). The `sources[].resource`
//!   edges among these are the **derivation graph**
//!   ([`Bundle::derived_from`]), which is how credibility propagates: "a
//!   consumer MAY read the `sources` of that concept recursively".
//!
//! Loading is **permissive** by design: files whose frontmatter cannot be
//! parsed are collected into [`Bundle::parse_errors`] rather than aborting the
//! load, and broken links are retained as edges to non-existent concepts.

use crate::computation::{AttestedComputation, ComputationSource};
use crate::concept_id::ConceptId;
use crate::date::{Date, DateTime};
use crate::document::Document;
use crate::error::{BundleError, DocumentError};
use crate::frontmatter::Frontmatter;
use crate::links;
use crate::provenance::Source;
use crate::trust::{Freshness, Status, TrustTier};
use crate::yaml::Value;
use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

/// Reserved filenames with defined meaning at any level. A file with one of
/// these names is never a concept, whatever frontmatter it contains.
pub const RESERVED_FILENAMES: [&str; 2] = ["index.md", "log.md"];

/// Whether a bundle that declares `declared` as its `okf_version` may be read
/// with the fallbacks for earlier versions.
///
/// That is the case for a bundle that declares 0.2 or earlier, or declares no
/// version. A declaration that does not read as a version number says nothing
/// about the bundle's age, so it is treated as no declaration.
///
/// The fallbacks are: a path field without a leading `/` that does not resolve
/// from its file may resolve from the bundle root, and the v0.1 `timestamp`
/// and `# Citations` forms.
#[must_use]
pub fn legacy_fallbacks_apply(declared: Option<&str>) -> bool {
    let parsed: Option<Vec<u64>> = declared.and_then(|version| {
        version
            .trim()
            .split('.')
            .map(|part| part.parse::<u64>().ok())
            .collect()
    });
    parsed.is_none_or(|version| version.as_slice() <= [0, 2].as_slice())
}

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
    /// The concept's `type`.
    #[must_use]
    pub fn type_(&self) -> Option<Cow<'_, str>> {
        self.document.frontmatter.type_()
    }

    /// The concept's `title`, falling back to the final segment of its id when
    /// none is given, as the spec permits.
    #[must_use]
    pub fn display_title(&self) -> String {
        self.document
            .frontmatter
            .title()
            .map_or_else(|| self.id.name().to_string(), std::borrow::Cow::into_owned)
    }

    /// The trust tier derived from the current `verified` events.
    #[must_use]
    pub fn trust_tier(&self) -> TrustTier {
        self.document.frontmatter.trust_tier()
    }

    /// The lifecycle `status`; absent means stable.
    #[must_use]
    pub fn status(&self) -> Status {
        self.document.frontmatter.status()
    }

    /// `true` when the concept counts as current knowledge: `stable` or
    /// `draft`. A deprecated concept, or one with an unknown status, is not.
    #[must_use]
    pub fn is_current(&self) -> bool {
        self.status().is_current()
    }

    /// Whether this concept is within its review window at `now`.
    #[must_use]
    pub fn freshness_at(&self, now: DateTime) -> Freshness {
        self.document.frontmatter.freshness_at(now)
    }

    /// Whether this concept must not be presented as fresh at `now`:
    /// `now >= stale_after`, or `stale_after` does not parse.
    #[must_use]
    pub fn is_stale_at(&self, now: DateTime) -> bool {
        self.document.frontmatter.is_stale_at(now)
    }

    /// Whether this concept is stale on `today`, evaluated at midnight UTC.
    #[must_use]
    pub fn is_stale_on(&self, today: Date) -> bool {
        self.document.frontmatter.is_stale_on(today)
    }

    /// The `sources` this concept derives from.
    #[must_use]
    pub fn sources(&self) -> Vec<Source> {
        self.document.frontmatter.sources()
    }

    /// The Attested Computation contract, when this concept is one.
    #[must_use]
    pub fn attested_computation(&self) -> Option<AttestedComputation> {
        self.document.attested_computation()
    }
}

/// What an in-bundle link target turned out to be.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LinkTarget {
    /// The target is a concept in the bundle. Only these links are edges of
    /// the bundle graph.
    Concept,
    /// The target exists in the bundle but is not a concept: an asset, a
    /// reserved `index.md` or `log.md`, or a markdown file without a `type`.
    /// Such a link is valid; it just does not refer to a concept.
    File,
    /// Nothing is there. Not malformed: it can be knowledge that nobody has
    /// written yet.
    Missing,
}

/// A link from a concept to another file in the bundle, after resolution.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedLink {
    /// The concept the link points at, or would point at if its target were
    /// one.
    pub target: ConceptId,
    /// Whether the target concept exists in the bundle. A `false` is allowed:
    /// see [`ResolvedLink::resolution`] to tell a link to an asset from a
    /// broken one.
    pub exists: bool,
    /// The link text.
    pub text: String,
    /// The raw link target as written.
    pub raw: String,
    /// What the target turned out to be.
    pub resolution: LinkTarget,
}

impl ResolvedLink {
    /// `true` when the target names nothing in the bundle.
    #[must_use]
    pub fn is_broken(&self) -> bool {
        self.resolution == LinkTarget::Missing
    }
}

/// Where a path field value leads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PathResolution {
    /// A URL, a scope descriptor, or a location outside the bundle root.
    /// There is nothing in the bundle to resolve, and nothing is read from the
    /// local file system.
    External,
    /// The value resolves to this file or directory in the bundle.
    Found(PathBuf),
    /// The value does not resolve from the directory of its file, but does
    /// resolve from the bundle root: the reading earlier OKF versions used for
    /// a path without a leading `/`. Only produced for a bundle that declares
    /// `okf_version` 0.2 or earlier, or no version. Writing the value with a
    /// leading `/` makes it resolve the same way everywhere.
    Fallback(PathBuf),
    /// The value names nothing in the bundle.
    Missing,
}

impl PathResolution {
    /// The file or directory the value resolved to, by either reading.
    #[must_use]
    pub fn path(&self) -> Option<&Path> {
        match self {
            Self::Found(path) | Self::Fallback(path) => Some(path),
            Self::External | Self::Missing => None,
        }
    }
}

/// A path field that resolves to a concept in the bundle: one kind of edge of
/// the bundle graph.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PathFieldEdge {
    /// The field the edge comes from, such as `superseded_by`,
    /// `executor.resource`, or `sources[0].resource`.
    pub field: String,
    /// The concept the field resolves to.
    pub target: ConceptId,
    /// The field value as written.
    pub raw: String,
}

impl PathFieldEdge {
    /// `true` when the edge comes from a `sources[].resource`, making it a
    /// derivation edge.
    #[must_use]
    pub fn is_source(&self) -> bool {
        self.field.starts_with("sources[")
    }
}

/// Why a markdown file in the bundle is not a concept.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum NonConceptReason {
    /// The file has no frontmatter block.
    NoFrontmatter,
    /// The frontmatter has no non-empty `type`.
    NoType,
}

impl std::fmt::Display for NonConceptReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::NoFrontmatter => "the file has no frontmatter",
            Self::NoType => "the frontmatter has no non-empty `type`",
        })
    }
}

/// A markdown file in the bundle that is not a concept. Consumers skip it, and
/// it does not affect conformance.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NonConcept {
    /// The file path on disk.
    pub path: PathBuf,
    /// Why the file is not a concept.
    pub reason: NonConceptReason,
}

/// The bundle metadata a bundle-root `index.md` carries in its frontmatter.
///
/// Every key is optional. They describe the bundle, not the concepts in it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BundleMetadata {
    /// The OKF version the bundle uses.
    pub okf_version: Option<String>,
    /// A display name for the bundle.
    pub title: Option<String>,
    /// One sentence about the scope of the bundle and when to use it. An agent
    /// with access to many bundles can read it to select one.
    pub description: Option<String>,
    /// The license of the bundle content: an SPDX expression, or a path to a
    /// license file in the bundle.
    pub license: Option<String>,
    /// The whole frontmatter block, including extension keys.
    pub frontmatter: Frontmatter,
}

impl BundleMetadata {
    /// Reads the metadata from the frontmatter of a bundle-root `index.md`.
    #[must_use]
    pub fn from_frontmatter(frontmatter: Frontmatter) -> Self {
        let string = |key: &str| {
            frontmatter
                .get(key)
                .and_then(Value::as_display_string)
                .filter(|value| !value.trim().is_empty())
        };
        Self {
            okf_version: string("okf_version"),
            title: string("title"),
            description: string("description"),
            license: string("license"),
            frontmatter,
        }
    }

    /// `true` when the `index.md` declares no bundle metadata at all.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.frontmatter.is_empty()
    }
}

/// A `sources` entry resolved against the bundle.
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
    /// The root with symbolic links resolved, used to keep every read inside
    /// the bundle. `None` only if the root could not be canonicalized.
    canonical_root: Option<PathBuf>,
    concepts: Vec<Concept>,
    index: HashMap<ConceptId, usize>,
    index_files: Vec<PathBuf>,
    log_files: Vec<PathBuf>,
    non_concepts: Vec<NonConcept>,
    parse_errors: Vec<(PathBuf, DocumentError)>,
    outbound: HashMap<ConceptId, Vec<ResolvedLink>>,
    backlinks: HashMap<ConceptId, Vec<ConceptId>>,
    sources: HashMap<ConceptId, Vec<ResolvedSource>>,
    derived_by: HashMap<ConceptId, Vec<ConceptId>>,
    path_edges: HashMap<ConceptId, Vec<PathFieldEdge>>,
    referenced_by: HashMap<ConceptId, Vec<ConceptId>>,
    /// The bundle metadata from the bundle-root `index.md` frontmatter. Read
    /// once at load time so the accessors can borrow.
    metadata: BundleMetadata,
}

/// What [`Bundle::load`] and [`Bundle::load_file`] read off the disk, before
/// the graphs are built.
#[derive(Default)]
struct Loaded {
    concepts: Vec<Concept>,
    index_files: Vec<PathBuf>,
    log_files: Vec<PathBuf>,
    non_concepts: Vec<NonConcept>,
    parse_errors: Vec<(PathBuf, DocumentError)>,
}

impl Loaded {
    fn push(&mut self, outcome: FileOutcome) {
        match outcome {
            FileOutcome::Index(p) => self.index_files.push(p),
            FileOutcome::Log(p) => self.log_files.push(p),
            FileOutcome::Concept(c) => self.concepts.push(c),
            FileOutcome::NonConcept(n) => self.non_concepts.push(n),
            FileOutcome::Error(p, e) => self.parse_errors.push((p, e)),
        }
    }
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

        let md_files = markdown_files(&root)?;

        // Parse every file in parallel. The work per file is
        // I/O-bound (`fs::read_to_string`) followed by CPU-bound
        // (`Document::parse`), so parallelizing across the file list scales
        // with cores on large bundles via `std::thread::scope`. Results are merged in chunk order so the
        // vectors below retain the deterministic sorted order callers rely on.
        let mut loaded = Loaded::default();
        for outcome in parse_files_parallel(&root, &md_files)? {
            loaded.push(outcome);
        }
        Ok(Self::assemble(root, loaded))
    }

    /// Loads a single markdown file into a bundle of at most one concept.
    ///
    /// The file's parent directory is used as the bundle root.
    ///
    /// # Errors
    ///
    /// Returns [`BundleError::Io`] on filesystem read errors.
    pub fn load_file(path: impl AsRef<Path>) -> Result<Self, BundleError> {
        let path = path.as_ref().to_path_buf();
        if !path.is_file() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("not a file: {}", path.display()),
            )
            .into());
        }
        let root = path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf();
        let mut loaded = Loaded::default();
        loaded.push(parse_one(&root, &path)?);
        Ok(Self::assemble(root, loaded))
    }

    /// Builds the graphs over what was read from disk.
    fn assemble(root: PathBuf, loaded: Loaded) -> Self {
        let Loaded {
            concepts,
            index_files,
            log_files,
            non_concepts,
            parse_errors,
        } = loaded;

        let mut index = HashMap::new();
        for (i, c) in concepts.iter().enumerate() {
            index.insert(c.id.clone(), i);
        }

        let metadata = read_metadata(&root);
        let canonical_root = fs::canonicalize(&root).ok();
        let resolver = Resolver {
            root: &root,
            canonical_root: canonical_root.as_deref(),
            legacy_fallbacks: legacy_fallbacks_apply(metadata.okf_version.as_deref()),
        };

        let (outbound, backlinks) = build_graph(&concepts, &index, &resolver);
        let (path_edges, referenced_by) = build_path_edges(&concepts, &index, &resolver);
        let (sources, derived_by) = build_derivation_graph(&concepts, &path_edges);

        Self {
            root,
            canonical_root,
            concepts,
            index,
            index_files,
            log_files,
            non_concepts,
            parse_errors,
            outbound,
            backlinks,
            sources,
            derived_by,
            path_edges,
            referenced_by,
            metadata,
        }
    }

    fn resolver(&self) -> Resolver<'_> {
        Resolver {
            root: &self.root,
            canonical_root: self.canonical_root.as_deref(),
            legacy_fallbacks: self.legacy_fallbacks_apply(),
        }
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

    /// Paths of all `index.md` files found.
    #[must_use]
    pub fn index_files(&self) -> &[PathBuf] {
        &self.index_files
    }

    /// Paths of all `log.md` files found.
    #[must_use]
    pub fn log_files(&self) -> &[PathBuf] {
        &self.log_files
    }

    /// Markdown files in the bundle that are not concepts: files with no
    /// frontmatter, or with no `type`. They are skipped, in path order, and do
    /// not affect conformance.
    #[must_use]
    pub fn non_concepts(&self) -> &[NonConcept] {
        &self.non_concepts
    }

    /// Files whose frontmatter block could not be parsed as a YAML mapping
    /// during loading. Unlike a file with no frontmatter, each of these is a
    /// conformance failure.
    #[must_use]
    pub fn parse_errors(&self) -> &[(PathBuf, DocumentError)] {
        &self.parse_errors
    }

    /// The resolved outbound cross-links from a concept.
    #[must_use]
    pub fn links_from(&self, id: &ConceptId) -> &[ResolvedLink] {
        self.outbound.get(id).map_or(&[], std::vec::Vec::as_slice)
    }

    /// The ids of concepts that link to the given concept ("cited by" /
    /// backlinks).
    #[must_use]
    pub fn backlinks(&self, id: &ConceptId) -> &[ConceptId] {
        self.backlinks.get(id).map_or(&[], std::vec::Vec::as_slice)
    }

    /// What a link written in the concept `from` points at: the concept id
    /// its target names, and whether that is a concept, another file, or
    /// nothing. See [`LinkTarget`].
    ///
    /// `None` for a link with no concept id to speak of: an external link, an
    /// in-document anchor, a target that ends in `/`, or one that leaves the
    /// bundle root.
    #[must_use]
    pub fn link_target(
        &self,
        from: &ConceptId,
        link: &links::Link,
    ) -> Option<(ConceptId, LinkTarget)> {
        self.resolver().resolve_link(&self.index, from, link)
    }

    /// The concept a link written in the concept `from` refers to, if it
    /// refers to one that is in the bundle.
    ///
    /// Unlike [`Link::resolve`](links::Link::resolve), which only does the
    /// path arithmetic, this looks at what is there: a link to an asset or to
    /// a directory is not a link to a concept, however its target is spelled.
    #[must_use]
    pub fn link_concept(&self, from: &ConceptId, link: &links::Link) -> Option<ConceptId> {
        self.link_target(from, link)
            .filter(|(_, resolution)| *resolution == LinkTarget::Concept)
            .map(|(id, _)| id)
    }

    /// All broken internal links in the bundle, as `(source, raw_target)`
    /// pairs: links whose target names nothing in the bundle. A link to an
    /// asset or to a markdown file that is not a concept is valid and is not
    /// listed. Broken links are permitted by the spec, so this is
    /// informational.
    #[must_use]
    pub fn broken_links(&self) -> Vec<(ConceptId, String)> {
        let mut out = Vec::new();
        for c in &self.concepts {
            for link in self.links_from(&c.id) {
                if link.is_broken() {
                    out.push((c.id.clone(), link.raw.clone()));
                }
            }
        }
        out
    }

    /// The bundle metadata declared in the frontmatter of the bundle-root
    /// `index.md`: `okf_version`, `title`, `description`, `license`, and any
    /// extension keys. Empty when there is no root `index.md`, it has no
    /// frontmatter, or it cannot be parsed.
    #[must_use]
    pub const fn metadata(&self) -> &BundleMetadata {
        &self.metadata
    }

    /// The declared OKF version from the bundle-root `index.md` frontmatter, if
    /// present (`okf_version`).
    ///
    /// Read at load time, so this is cheap to call repeatedly. A consumer
    /// that does not know the declared version SHOULD try best-effort
    /// consumption rather than refusing the bundle, so this is reported, never
    /// enforced.
    ///
    /// Returns `None` whether the root `index.md` is absent, unreadable, or
    /// lacks the key; a malformed root `index.md` is reported separately by
    /// `validate_bundle` (in the okf-validator crate).
    #[must_use]
    pub fn okf_version(&self) -> Option<&str> {
        self.metadata.okf_version.as_deref()
    }

    /// Whether this bundle may be read with the fallbacks for earlier OKF
    /// versions: it declares `okf_version` 0.2 or earlier, or no version. See
    /// [`legacy_fallbacks_apply`].
    #[must_use]
    pub fn legacy_fallbacks_apply(&self) -> bool {
        legacy_fallbacks_apply(self.okf_version())
    }

    /// The concept's `sources` entries, each resolved against the bundle.
    #[must_use]
    pub fn sources_of(&self, id: &ConceptId) -> &[ResolvedSource] {
        self.sources.get(id).map_or(&[], std::vec::Vec::as_slice)
    }

    /// The path fields of a concept that resolve to another concept in the
    /// bundle: its outgoing path-field edges in the bundle graph.
    #[must_use]
    pub fn path_edges_from(&self, id: &ConceptId) -> &[PathFieldEdge] {
        self.path_edges.get(id).map_or(&[], std::vec::Vec::as_slice)
    }

    /// The concepts that have a path field resolving to this one: the reverse
    /// of [`Bundle::path_edges_from`].
    ///
    /// Together with [`Bundle::backlinks`] this is everything that refers to a
    /// concept, which is what a move or a removal has to keep intact.
    #[must_use]
    pub fn referenced_by(&self, id: &ConceptId) -> &[ConceptId] {
        self.referenced_by
            .get(id)
            .map_or(&[], std::vec::Vec::as_slice)
    }

    /// The concept that replaces a deprecated one: its `superseded_by`,
    /// resolved against the bundle.
    ///
    /// `None` unless the concept has `status: deprecated`, since the field has
    /// a meaning only then, and unless the path resolves to a concept in the
    /// bundle.
    #[must_use]
    pub fn superseded_by(&self, id: &ConceptId) -> Option<&ConceptId> {
        if !self.get(id)?.status().is_deprecated() {
            return None;
        }
        self.path_edges_from(id)
            .iter()
            .find(|edge| edge.field == "superseded_by")
            .map(|edge| &edge.target)
    }

    /// The current concept to give in place of `id`, following
    /// `superseded_by` from a deprecated concept to its replacement, and
    /// onward if that one is deprecated too.
    ///
    /// Returns the concept itself when it is current, and `None` when the
    /// chain ends at a concept that is not current, or loops.
    #[must_use]
    pub fn current_replacement(&self, id: &ConceptId) -> Option<&Concept> {
        let mut seen: HashSet<&ConceptId> = HashSet::new();
        let mut concept = self.get(id)?;
        loop {
            if concept.is_current() {
                return Some(concept);
            }
            if !seen.insert(&concept.id) {
                return None;
            }
            concept = self.get(self.superseded_by(&concept.id)?)?;
        }
    }

    /// The concepts this one derives from: the `sources[].resource` entries
    /// that name another concept in this bundle.
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

    /// Every `Attested Computation` concept in the bundle.
    ///
    /// This is the discovery path: a consumer reaches a
    /// computation by type, or by following a link from a concept that uses it.
    pub fn attested_computations(&self) -> impl Iterator<Item = &Concept> {
        self.concepts_of_type(crate::computation::ATTESTED_COMPUTATION_TYPE)
    }

    /// A tag index synthesized by scanning frontmatter, tag to concept ids.
    ///
    /// OKF does not specify a file format for aggregating documents by
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

    /// Every concept that is stale at `now`: `now >= stale_after`.
    #[must_use]
    pub fn stale_at(&self, now: DateTime) -> Vec<&Concept> {
        self.concepts
            .iter()
            .filter(|c| c.is_stale_at(now))
            .collect()
    }

    /// Every concept that is stale on `today`: `today >= stale_after`.
    #[must_use]
    pub fn stale_on(&self, today: Date) -> Vec<&Concept> {
        self.concepts
            .iter()
            .filter(|c| c.is_stale_on(today))
            .collect()
    }

    /// Resolves a path field of the concept `from` and reports how.
    ///
    /// The value resolves from the bundle root when it starts with `/`, and
    /// otherwise from the directory of the concept's file. A value that still
    /// names nothing may, in a bundle that declares OKF 0.2 or earlier or no
    /// version, resolve from the bundle root instead; that is reported as
    /// [`PathResolution::Fallback`] so a producer can be told to add the `/`.
    ///
    /// Nothing outside the bundle root is ever read: a URL, a path that climbs
    /// out of the root, and a symbolic link that leads out of it are all
    /// [`PathResolution::External`].
    #[must_use]
    pub fn resolve_path(&self, from: &ConceptId, raw: &str) -> PathResolution {
        let dir = from.directory().to_vec();
        self.resolver().resolve(&dir, raw)
    }

    /// As [`Bundle::resolve_path`], for a value in a file that is not a
    /// concept: `dir` is the bundle-relative directory of that file, empty for
    /// the bundle root. This is how the `license` of a bundle-root `index.md`
    /// resolves.
    #[must_use]
    pub fn resolve_path_in(&self, dir: &[String], raw: &str) -> PathResolution {
        self.resolver().resolve(dir, raw)
    }

    /// Resolves a path field to a file or directory inside the bundle.
    ///
    /// Returns `None` for a URL, a scope descriptor, or a path that names
    /// nothing. Unlike concept links, these fields routinely point at
    /// non-markdown files such as `/references/attesters/revenue.py`. See
    /// [`Bundle::resolve_path`] to tell the cases apart.
    #[must_use]
    pub fn resolve_path_field(&self, from: &ConceptId, raw: &str) -> Option<PathBuf> {
        self.resolve_path(from, raw).path().map(Path::to_path_buf)
    }

    /// The `/`-rooted spelling of a path field that only resolves when read
    /// from the bundle root.
    ///
    /// Earlier OKF versions wrote such paths without the leading `/`
    /// (`references/skills/run-on-bq.md` on a concept in `computations/`). A
    /// path without it now resolves from the directory of its file, so the
    /// value either depends on the fallback for older bundles or does not
    /// resolve at all. Returns `Some("/references/skills/run-on-bq.md")` for
    /// such a value, whatever version the bundle declares, and `None` when the
    /// value resolves as written, is external, or names nothing either way.
    #[must_use]
    pub fn rooted_path_suggestion(&self, from: &ConceptId, raw: &str) -> Option<String> {
        let dir = from.directory().to_vec();
        let resolver = self.resolver();
        let primary = links::path_candidates_in(raw, &dir);
        if primary.is_empty() || !matches!(resolver.first_on_disk(&primary), OnDisk::Absent) {
            return None;
        }
        let legacy = links::legacy_root_path_candidates(raw);
        if !matches!(resolver.first_on_disk(&legacy), OnDisk::Inside(_)) {
            return None;
        }
        // Keep the spelling the author used (encoding, fragment, query) and
        // only anchor it at the root.
        let trimmed = raw.trim();
        let path = links::path_part(trimmed);
        let suffix = &trimmed[path.len()..];
        let rooted: Vec<&str> = path
            .split('/')
            .filter(|segment| !segment.is_empty() && *segment != ".")
            .collect();
        Some(format!("/{}{suffix}", rooted.join("/")))
    }

    /// Every path field in the bundle that only resolves when read from the
    /// bundle root, as `(concept, field, value as written, rooted spelling)`.
    ///
    /// An empty result means no concept depends on the fallback for bundles
    /// from earlier OKF versions, so the bundle reads the same whether or not
    /// that fallback applies.
    #[must_use]
    pub fn legacy_path_fields(&self) -> Vec<(ConceptId, String, String, String)> {
        let mut out = Vec::new();
        for concept in &self.concepts {
            for (field, raw) in concept.document.frontmatter.path_fields() {
                if let Some(rooted) = self.rooted_path_suggestion(&concept.id, &raw) {
                    out.push((concept.id.clone(), field, raw, rooted));
                }
            }
        }
        out
    }

    /// Whether a consumer may show a value from this concept as attested.
    ///
    /// That needs an Attested Computation with a `runtime`, an `attester`, and
    /// a computation that can be found: a fenced block in the `Computation`
    /// section, or a `computation` path that names a file in the bundle. An
    /// `attester.resource` that is a path must lead somewhere too, since a
    /// check that is not there cannot be run. A concept that is not
    /// attestable is still readable.
    #[must_use]
    pub fn is_attestable(&self, id: &ConceptId) -> bool {
        let Some(contract) = self.get(id).and_then(Concept::attested_computation) else {
            return false;
        };
        if !contract.is_attestable() {
            return false;
        }
        let attester_found = contract
            .attester
            .as_ref()
            .and_then(|attester| attester.resource.as_deref())
            .is_some_and(|raw| self.resolve_path(id, raw) != PathResolution::Missing);
        let computation_found = match &contract.computation {
            ComputationSource::File(path) => self
                .resolve_path(id, path)
                .path()
                .is_some_and(Path::is_file),
            _ => true,
        };
        attester_found && computation_found
    }
}

/// The per-file result of loading a single markdown path.
enum FileOutcome {
    Index(PathBuf),
    Log(PathBuf),
    Concept(Concept),
    NonConcept(NonConcept),
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
/// collected as [`FileOutcome::Error`] for the permissive-load path.
fn parse_one(root: &Path, path: &Path) -> Result<FileOutcome, std::io::Error> {
    let filename = path
        .file_name()
        .map_or_else(String::new, |f| f.to_string_lossy().into_owned());
    match filename.as_str() {
        "index.md" => Ok(FileOutcome::Index(path.to_path_buf())),
        "log.md" => Ok(FileOutcome::Log(path.to_path_buf())),
        _ => {
            let text = match fs::read_to_string(path) {
                Ok(text) => text,
                // One file in another encoding is a problem with that file,
                // to be reported against it, not a reason to give up on the
                // whole bundle.
                Err(e) if e.kind() == std::io::ErrorKind::InvalidData => {
                    return Ok(FileOutcome::Error(
                        path.to_path_buf(),
                        DocumentError::NotUtf8,
                    ));
                }
                Err(e) => return Err(e),
            };
            let document = match Document::parse(&text) {
                Ok(document) => document,
                Err(e) => return Ok(FileOutcome::Error(path.to_path_buf(), e)),
            };
            // A file without frontmatter, or without `type`, is not a concept.
            let reason = if !Document::has_frontmatter_block(&text) {
                Some(NonConceptReason::NoFrontmatter)
            } else if !document.is_concept() {
                Some(NonConceptReason::NoType)
            } else {
                None
            };
            if let Some(reason) = reason {
                return Ok(FileOutcome::NonConcept(NonConcept {
                    path: path.to_path_buf(),
                    reason,
                }));
            }
            Ok(match ConceptId::from_path(root, path) {
                Ok(id) => FileOutcome::Concept(Concept {
                    id,
                    path: path.to_path_buf(),
                    document,
                }),
                Err(e) => FileOutcome::Error(path.to_path_buf(), e.into()),
            })
        }
    }
}

/// `true` when `path` exists but leads out of `root`, through a symbolic link.
/// Such a file is not part of the bundle: it is neither read nor written.
pub(crate) fn leads_outside(root: &Path, path: &Path) -> bool {
    match (fs::canonicalize(root), fs::canonicalize(path)) {
        (Ok(root), Ok(resolved)) => !resolved.starts_with(root),
        _ => false,
    }
}

/// Reads the bundle metadata from the bundle-root `index.md` frontmatter.
/// Empty for a missing file, an unparseable one, or one that is a symbolic
/// link out of the bundle root.
fn read_metadata(root: &Path) -> BundleMetadata {
    let index = root.join("index.md");
    if leads_outside(root, &index) {
        return BundleMetadata::default();
    }
    fs::read_to_string(index)
        .ok()
        .and_then(|text| Document::parse(&text).ok())
        .map_or_else(BundleMetadata::default, |doc| {
            BundleMetadata::from_frontmatter(doc.frontmatter)
        })
}

/// Every markdown file that is part of the bundle rooted at `root`, sorted by
/// path.
///
/// Files and directories whose names start with `.` are not part of a bundle
/// and are skipped. A symbolic link to a markdown file is followed only when
/// it stays inside the bundle root; a symbolic link to a directory is never
/// descended into, which also keeps a link cycle from looping the walk.
///
/// # Errors
///
/// Returns the underlying [`std::io::Error`] from the directory walk.
pub fn markdown_files(root: &Path) -> std::io::Result<Vec<PathBuf>> {
    let canonical_root = fs::canonicalize(root).ok();
    let mut out = Vec::new();
    collect_markdown(root, canonical_root.as_deref(), &mut out)?;
    out.sort();
    Ok(out)
}

/// Recursively collects `*.md` file paths under `dir`.
fn collect_markdown(
    dir: &Path,
    canonical_root: Option<&Path>,
    out: &mut Vec<PathBuf>,
) -> std::io::Result<()> {
    let mut entries: Vec<_> = fs::read_dir(dir)?.collect::<Result<_, _>>()?;
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for entry in entries {
        if entry.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        let path = entry.path();
        let file_type = entry.file_type()?;
        let is_markdown = path.extension().is_some_and(|e| e == "md");
        if file_type.is_dir() {
            collect_markdown(&path, canonical_root, out)?;
        } else if !is_markdown {
            // An asset. It can be linked to, but it is never read here.
        } else if file_type.is_file() {
            out.push(path);
        } else if file_type.is_symlink()
            && let Some(root) = canonical_root
            && let Ok(resolved) = fs::canonicalize(&path)
            && resolved.is_file()
            && resolved.starts_with(root)
        {
            out.push(path);
        }
    }
    Ok(())
}

/// Resolves link targets and path fields against the files of a bundle,
/// without ever reading outside its root.
struct Resolver<'a> {
    root: &'a Path,
    canonical_root: Option<&'a Path>,
    legacy_fallbacks: bool,
}

/// `true` when `actual`, the spelling a path has on disk, matches the
/// `/`-separated `requested` path in everything but letter case.
///
/// Any other difference means a symbolic link was followed, which is fine.
fn differs_only_by_case(actual: &Path, requested: &str) -> bool {
    let actual: Vec<String> = actual
        .components()
        .map(|component| component.as_os_str().to_string_lossy().into_owned())
        .collect();
    let requested: Vec<&str> = requested.split('/').collect();
    actual.len() == requested.len()
        && actual.iter().zip(&requested).any(|(a, r)| a != r)
        && actual
            .iter()
            .zip(&requested)
            .all(|(a, r)| a.eq_ignore_ascii_case(r))
}

/// What a bundle-relative path names on disk.
enum OnDisk {
    /// A file or directory inside the bundle root.
    Inside(PathBuf),
    /// Something that exists but leads out of the bundle root through a
    /// symbolic link. Never read.
    Outside,
    /// Nothing.
    Absent,
}

impl Resolver<'_> {
    fn on_disk(&self, relative: &str) -> OnDisk {
        let path = self.root.join(relative);
        if !path.exists() {
            return OnDisk::Absent;
        }
        match (self.canonical_root, fs::canonicalize(&path)) {
            (Some(root), Ok(resolved)) => match resolved.strip_prefix(root) {
                // Paths are case-sensitive. A file system that is not would
                // find `Tables/Orders.md` for `tables/orders.md`, and the
                // same bundle would then read differently on another
                // machine. The canonical path carries the real spelling.
                Ok(actual)
                    if differs_only_by_case(actual, relative)
                        && !path.symlink_metadata().is_ok_and(|meta| meta.is_symlink()) =>
                {
                    OnDisk::Absent
                }
                Ok(_) => OnDisk::Inside(path),
                Err(_) => OnDisk::Outside,
            },
            // Without a canonical root there is nothing to compare against.
            (None, _) => OnDisk::Inside(path),
            (Some(_), Err(_)) => OnDisk::Outside,
        }
    }

    /// The first of `candidates` that exists inside the bundle, or `Outside`
    /// if one exists but escapes it.
    fn first_on_disk(&self, candidates: &[String]) -> OnDisk {
        let mut outside = false;
        for candidate in candidates {
            match self.on_disk(candidate) {
                OnDisk::Inside(path) => return OnDisk::Inside(path),
                OnDisk::Outside => outside = true,
                OnDisk::Absent => {}
            }
        }
        if outside {
            OnDisk::Outside
        } else {
            OnDisk::Absent
        }
    }

    fn resolve(&self, dir: &[String], raw: &str) -> PathResolution {
        let candidates = links::path_candidates_in(raw, dir);
        if candidates.is_empty() {
            return PathResolution::External;
        }
        match self.first_on_disk(&candidates) {
            OnDisk::Inside(path) => return PathResolution::Found(path),
            OnDisk::Outside => return PathResolution::External,
            OnDisk::Absent => {}
        }
        if self.legacy_fallbacks
            && let OnDisk::Inside(path) =
                self.first_on_disk(&links::legacy_root_path_candidates(raw))
        {
            return PathResolution::Fallback(path);
        }
        PathResolution::Missing
    }

    /// What an in-bundle link of the concept `from` points at, with the
    /// concept id its target names (or would name).
    ///
    /// The target is looked up as a file first. If it exists, the link refers
    /// to a concept only when that file is one: a link to an asset, to a
    /// directory, or to a markdown file without a `type` is valid, but does
    /// not refer to a concept. Only when the target names nothing is it also
    /// tried without its `.md` suffix, so that `[orders](orders)` still finds
    /// `orders.md`.
    ///
    /// `None` for a link that has no concept id to speak of: an external
    /// link, an in-document anchor, a target that ends in `/`, or one that
    /// leaves the bundle root.
    fn resolve_link(
        &self,
        index: &HashMap<ConceptId, usize>,
        from: &ConceptId,
        link: &links::Link,
    ) -> Option<(ConceptId, LinkTarget)> {
        // A percent-encoded target has two readings; the literal one comes
        // first, and is the one a broken link is reported under.
        let candidates = link.resolve_all(from);
        let fallback = candidates.first()?.clone();
        match self.first_on_disk(&link.path_candidates(from)) {
            OnDisk::Inside(path) => {
                let concept = ConceptId::from_path(self.root, &path)
                    .ok()
                    .filter(|_| path.is_file() && path.extension().is_some_and(|e| e == "md"))
                    .filter(|id| index.contains_key(id));
                Some(concept.map_or((fallback, LinkTarget::File), |id| (id, LinkTarget::Concept)))
            }
            OnDisk::Outside => Some((fallback, LinkTarget::File)),
            OnDisk::Absent => Some(
                candidates
                    .into_iter()
                    .find(|id| index.contains_key(id))
                    .map_or((fallback, LinkTarget::Missing), |id| {
                        (id, LinkTarget::Concept)
                    }),
            ),
        }
    }

    /// The concept a path field of `from` resolves to, if it resolves to one
    /// that is in the bundle.
    ///
    /// With `allow_bare_id`, a value that names no file is also matched
    /// against concept ids, so that `resource: /tables/orders` finds
    /// `tables/orders.md`. That is for `sources` entries, where a producer
    /// means a concept; a top-level `resource` such as `orders` is an opaque
    /// asset identifier and must not be read as a link to a sibling.
    fn concept_for_field(
        &self,
        index: &HashMap<ConceptId, usize>,
        from: &ConceptId,
        raw: &str,
        allow_bare_id: bool,
    ) -> Option<ConceptId> {
        let dir = from.directory().to_vec();
        match self.resolve(&dir, raw) {
            PathResolution::Found(path) | PathResolution::Fallback(path) => {
                ConceptId::from_path(self.root, &path)
                    .ok()
                    .filter(|_| path.extension().is_some_and(|e| e == "md"))
                    .filter(|id| index.contains_key(id))
            }
            PathResolution::External => None,
            PathResolution::Missing if !allow_bare_id => None,
            // A path written without its `.md` suffix names no file, but it
            // is unambiguous when a concept of that id exists.
            PathResolution::Missing => {
                let mut candidates = links::path_candidates_in(raw, &dir);
                if self.legacy_fallbacks {
                    candidates.extend(links::legacy_root_path_candidates(raw));
                }
                candidates
                    .iter()
                    .filter_map(|candidate| ConceptId::parse(candidate).ok())
                    .find(|id| index.contains_key(id))
            }
        }
    }
}

/// Builds the outbound link and backlink maps for all concepts.
fn build_graph(
    concepts: &[Concept],
    index: &HashMap<ConceptId, usize>,
    resolver: &Resolver,
) -> (
    HashMap<ConceptId, Vec<ResolvedLink>>,
    HashMap<ConceptId, Vec<ConceptId>>,
) {
    let mut outbound: HashMap<ConceptId, Vec<ResolvedLink>> = HashMap::new();
    let mut backlinks: HashMap<ConceptId, Vec<ConceptId>> = HashMap::new();

    for c in concepts {
        let mut links_out = Vec::new();
        for link in c.document.links() {
            let Some((target, resolution)) = resolver.resolve_link(index, &c.id, &link) else {
                continue;
            };
            let exists = resolution == LinkTarget::Concept;
            if exists {
                let entry = backlinks.entry(target.clone()).or_default();
                if !entry.contains(&c.id) {
                    entry.push(c.id.clone());
                }
            }
            links_out.push(ResolvedLink {
                target,
                exists,
                text: link.text,
                raw: link.target,
                resolution,
            });
        }
        outbound.insert(c.id.clone(), links_out);
    }

    (outbound, backlinks)
}

/// Builds the path-field edges: every path field that resolves to another
/// concept in the bundle, and the reverse map.
fn build_path_edges(
    concepts: &[Concept],
    index: &HashMap<ConceptId, usize>,
    resolver: &Resolver,
) -> (
    HashMap<ConceptId, Vec<PathFieldEdge>>,
    HashMap<ConceptId, Vec<ConceptId>>,
) {
    let mut edges: HashMap<ConceptId, Vec<PathFieldEdge>> = HashMap::new();
    let mut referenced_by: HashMap<ConceptId, Vec<ConceptId>> = HashMap::new();

    for c in concepts {
        let mut out = Vec::new();
        for (field, raw) in c.document.frontmatter.path_fields() {
            let Some(target) = resolver
                .concept_for_field(index, &c.id, &raw, field.starts_with("sources["))
                .filter(|target| target != &c.id)
            else {
                continue;
            };
            let entry = referenced_by.entry(target.clone()).or_default();
            if !entry.contains(&c.id) {
                entry.push(c.id.clone());
            }
            out.push(PathFieldEdge { field, target, raw });
        }
        if !out.is_empty() {
            edges.insert(c.id.clone(), out);
        }
    }

    (edges, referenced_by)
}

/// Builds the derivation graph: every concept's `sources` entries, with the
/// ones naming another concept in the bundle resolved to its id.
fn build_derivation_graph(
    concepts: &[Concept],
    path_edges: &HashMap<ConceptId, Vec<PathFieldEdge>>,
) -> (
    HashMap<ConceptId, Vec<ResolvedSource>>,
    HashMap<ConceptId, Vec<ConceptId>>,
) {
    let mut sources: HashMap<ConceptId, Vec<ResolvedSource>> = HashMap::new();
    let mut derived_by: HashMap<ConceptId, Vec<ConceptId>> = HashMap::new();

    for c in concepts {
        let edges = path_edges.get(&c.id).map_or(&[][..], Vec::as_slice);
        // `Frontmatter::sources` leaves out entries without a `resource`,
        // while the edge names count every entry, so match on the raw value.
        let entries: Vec<ResolvedSource> = c
            .sources()
            .into_iter()
            .map(|source| {
                let concept = source.resource.as_deref().and_then(|raw| {
                    edges
                        .iter()
                        .find(|edge| edge.is_source() && edge.raw == raw)
                        .map(|edge| edge.target.clone())
                });
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
