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
use crate::date::Date;
use crate::document::Document;
use crate::error::{BundleError, DocumentError};
use crate::links;
use crate::provenance::Source;
use crate::trust::{Status, TrustTier};
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
    pub fn type_(&self) -> Option<String> {
        self.document.frontmatter.type_()
    }

    /// The concept's `title`, falling back to the final segment of its id when
    /// none is given, as §4.1 permits.
    pub fn display_title(&self) -> String {
        self.document
            .frontmatter
            .title()
            .unwrap_or_else(|| self.id.name().to_string())
    }

    /// The trust tier derived from `verified` (§5.3).
    pub fn trust_tier(&self) -> TrustTier {
        self.document.frontmatter.trust_tier()
    }

    /// The lifecycle `status`; absent means stable (§5.4).
    pub fn status(&self) -> Status {
        self.document.frontmatter.status()
    }

    /// Whether `today >= stale_after` (§5.5).
    pub fn is_stale_on(&self, today: Date) -> bool {
        self.document.frontmatter.is_stale_on(today)
    }

    /// The `sources` this concept derives from (§5.1).
    pub fn sources(&self) -> Vec<Source> {
        self.document.frontmatter.sources()
    }

    /// The Attested Computation contract, when this concept is one (§10).
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
}

impl Bundle {
    /// Loads a bundle from a directory tree.
    ///
    /// Returns an error only for I/O failures or a non-directory root. Per-file
    /// parse failures are recorded in [`Bundle::parse_errors`].
    pub fn load(root: impl AsRef<Path>) -> Result<Bundle, BundleError> {
        let root = root.as_ref().to_path_buf();
        if !root.is_dir() {
            return Err(BundleError::NotADirectory(root));
        }

        let mut md_files = Vec::new();
        collect_markdown(&root, &mut md_files)?;
        md_files.sort();

        let mut concepts = Vec::new();
        let mut index_files = Vec::new();
        let mut log_files = Vec::new();
        let mut parse_errors = Vec::new();

        for path in md_files {
            let filename = path
                .file_name()
                .map(|f| f.to_string_lossy().to_string())
                .unwrap_or_default();
            match filename.as_str() {
                "index.md" => index_files.push(path),
                "log.md" => log_files.push(path),
                _ => {
                    let text = fs::read_to_string(&path)?;
                    match Document::parse(&text) {
                        Ok(document) => match ConceptId::from_path(&root, &path) {
                            Ok(id) => concepts.push(Concept { id, path, document }),
                            Err(e) => parse_errors
                                .push((path, DocumentError::MissingKeys(vec![e.to_string()]))),
                        },
                        Err(e) => parse_errors.push((path, e)),
                    }
                }
            }
        }

        let mut index = HashMap::new();
        for (i, c) in concepts.iter().enumerate() {
            index.insert(c.id.clone(), i);
        }

        let (outbound, backlinks) = build_graph(&concepts, &index);
        let (sources, derived_by) = build_derivation_graph(&concepts, &index);

        Ok(Bundle {
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
        })
    }

    /// The bundle's root directory.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// All successfully parsed concepts, in path order.
    pub fn concepts(&self) -> &[Concept] {
        &self.concepts
    }

    /// Number of concepts.
    pub fn len(&self) -> usize {
        self.concepts.len()
    }

    /// `true` if the bundle has no concepts.
    pub fn is_empty(&self) -> bool {
        self.concepts.is_empty()
    }

    /// Looks up a concept by id.
    pub fn get(&self, id: &ConceptId) -> Option<&Concept> {
        self.index.get(id).map(|&i| &self.concepts[i])
    }

    /// `true` if a concept with this id exists.
    pub fn contains(&self, id: &ConceptId) -> bool {
        self.index.contains_key(id)
    }

    /// Paths of all `index.md` files found (§6).
    pub fn index_files(&self) -> &[PathBuf] {
        &self.index_files
    }

    /// Paths of all `log.md` files found (§7).
    pub fn log_files(&self) -> &[PathBuf] {
        &self.log_files
    }

    /// Files whose frontmatter could not be parsed during loading.
    pub fn parse_errors(&self) -> &[(PathBuf, DocumentError)] {
        &self.parse_errors
    }

    /// The resolved outbound cross-links from a concept.
    pub fn links_from(&self, id: &ConceptId) -> &[ResolvedLink] {
        self.outbound.get(id).map(|v| v.as_slice()).unwrap_or(&[])
    }

    /// The ids of concepts that link to the given concept ("cited by" / §
    /// backlinks).
    pub fn backlinks(&self, id: &ConceptId) -> &[ConceptId] {
        self.backlinks.get(id).map(|v| v.as_slice()).unwrap_or(&[])
    }

    /// All broken internal links in the bundle, as `(source, raw_target)`
    /// pairs. Broken links are permitted by the spec (§6.1), so this is
    /// informational.
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
    /// A consumer that does not understand the declared version SHOULD attempt
    /// best-effort consumption rather than refusing the bundle, so this is
    /// reported, never enforced.
    pub fn okf_version(&self) -> Option<String> {
        let root_index = self.root.join("index.md");
        let text = fs::read_to_string(&root_index).ok()?;
        let doc = Document::parse(&text).ok()?;
        doc.frontmatter
            .get("okf_version")
            .and_then(crate::yaml::Value::as_display_string)
    }

    /// The concept's `sources` entries, each resolved against the bundle.
    pub fn sources_of(&self, id: &ConceptId) -> &[ResolvedSource] {
        self.sources.get(id).map(|v| v.as_slice()).unwrap_or(&[])
    }

    /// The concepts this one derives from: the `sources[].resource` entries
    /// that name another concept in this bundle (§5.1).
    ///
    /// Following these recursively is how a consumer lets credibility
    /// propagate; external leaf sources carry only their intrinsic signals.
    pub fn derived_from(&self, id: &ConceptId) -> Vec<&ConceptId> {
        self.sources_of(id)
            .iter()
            .filter_map(|s| s.concept.as_ref())
            .collect()
    }

    /// The reverse of [`Bundle::derived_from`]: concepts that cite this one as
    /// a source.
    pub fn derives(&self, id: &ConceptId) -> &[ConceptId] {
        self.derived_by.get(id).map(|v| v.as_slice()).unwrap_or(&[])
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
    pub fn tags(&self) -> BTreeMap<String, Vec<ConceptId>> {
        let mut out: BTreeMap<String, Vec<ConceptId>> = BTreeMap::new();
        for c in &self.concepts {
            for tag in c.document.frontmatter.tags() {
                out.entry(tag).or_default().push(c.id.clone());
            }
        }
        out
    }

    /// Every concept that is stale on `today`: `today >= stale_after` (§5.5).
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
    pub fn resolve_path_field(&self, from: &ConceptId, raw: &str) -> Option<PathBuf> {
        links::field_path_candidates(raw, from)
            .into_iter()
            .map(|rel| self.root.join(rel))
            .find(|p| p.exists())
    }
}

/// Recursively collects `*.md` file paths under `dir`.
fn collect_markdown(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), BundleError> {
    let mut entries: Vec<_> = fs::read_dir(dir)?.collect::<Result<_, _>>()?;
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let path = entry.path();
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            collect_markdown(&path, out)?;
        } else if file_type.is_file() && path.extension().map(|e| e == "md").unwrap_or(false) {
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
