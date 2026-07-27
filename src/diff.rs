//! Bundle-level diff: an OKF-semantics diff between two [`Bundle`]s.
//!
//! [`bundle_diff`] reports concepts added, removed, renamed (detected by
//! content hash), frontmatter key changes, trust-tier/status changes, and links
//! broken or mended between two snapshots. It is a semantic diff, not a raw
//! text diff: two concepts whose frontmatter and body parse to the same value
//! are equal here even if their source text differs in whitespace.
//!
//! The rename heuristic hashes a concept's body together with its `type`,
//! `title`, and `description` (the identifying frontmatter fields that do not
//! depend on the concept's id). A removed and an added concept sharing that
//! hash are reported as a rename rather than as separate add and remove. The
//! heuristic is best-effort: a move that also edits the body, or one whose body
//! contains self-referential links, will not be detected.

use crate::bundle::{Bundle, Concept};
use crate::concept_id::ConceptId;
use crate::trust::{Status, TrustTier};
use crate::yaml::Value;
use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeSet, HashMap};
use std::hash::{Hash, Hasher};

/// A rename detected by matching content hash between a removed and an added
/// concept.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rename {
    /// The concept's id in the first bundle.
    pub from: ConceptId,
    /// The concept's id in the second bundle.
    pub to: ConceptId,
}

/// Frontmatter key changes for a concept present in both bundles.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FrontmatterChange {
    /// The concept the change applies to.
    pub id: ConceptId,
    /// Keys present in the second bundle but not the first.
    pub added: Vec<String>,
    /// Keys present in the first bundle but not the second.
    pub removed: Vec<String>,
    /// Keys present in both but with a different value, as
    /// `(key, old display form, new display form)`.
    pub changed: Vec<(String, String, String)>,
}

/// A trust tier or status change for a concept present in both bundles.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrustChange {
    /// The concept the change applies to.
    pub id: ConceptId,
    /// The trust tier transition, when it changed.
    pub tier: Option<(TrustTier, TrustTier)>,
    /// The status transition, when it changed.
    pub status: Option<(Status, Status)>,
}

/// A bundle-level diff.
///
/// Build one with [`bundle_diff`] and either read its fields directly or print
/// it with its [`Display`](std::fmt::Display) implementation, which is what
/// `okf diff` uses.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BundleDiff {
    /// Concepts present in the second bundle and absent from the first, after
    /// subtracting renames.
    pub added: Vec<ConceptId>,
    /// Concepts present in the first bundle and absent from the second, after
    /// subtracting renames.
    pub removed: Vec<ConceptId>,
    /// Concepts whose id changed but whose content hash did not.
    pub renamed: Vec<Rename>,
    /// Per-concept frontmatter key changes, for ids present in both bundles.
    pub frontmatter: Vec<FrontmatterChange>,
    /// Per-concept trust-tier/status changes, for ids present in both bundles.
    pub trust: Vec<TrustChange>,
    /// Links broken in the first bundle that are resolved in the second, as
    /// `(source id, raw target as written)`.
    pub mended_links: Vec<(ConceptId, String)>,
    /// Links resolved in the first bundle that are broken in the second, as
    /// `(source id, raw target as written)`.
    pub broken_links: Vec<(ConceptId, String)>,
}

impl BundleDiff {
    /// `true` when the two bundles are semantically identical.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.added.is_empty()
            && self.removed.is_empty()
            && self.renamed.is_empty()
            && self.frontmatter.is_empty()
            && self.trust.is_empty()
            && self.mended_links.is_empty()
            && self.broken_links.is_empty()
    }
}

/// Computes the OKF-semantics diff between two bundles.
///
/// Both bundles are loaded fully before this call; the diff itself does no
/// filesystem access.
#[must_use]
pub fn bundle_diff(a: &Bundle, b: &Bundle) -> BundleDiff {
    let a_ids: BTreeSet<ConceptId> = a.concepts().iter().map(|c| c.id.clone()).collect();
    let b_ids: BTreeSet<ConceptId> = b.concepts().iter().map(|c| c.id.clone()).collect();

    let removed: Vec<ConceptId> = a_ids.difference(&b_ids).cloned().collect();
    let added: Vec<ConceptId> = b_ids.difference(&a_ids).cloned().collect();

    // Match renames by content hash among the removed and added concepts. A
    // removed concept whose hash matches an added concept's is reported as a
    // rename and dropped from the add/remove lists.
    let mut removed_by_hash: HashMap<u64, Vec<ConceptId>> = HashMap::new();
    for id in &removed {
        if let Some(c) = a.get(id) {
            removed_by_hash
                .entry(content_hash(c))
                .or_default()
                .push(id.clone());
        }
    }
    let mut consumed_removed: BTreeSet<ConceptId> = BTreeSet::new();
    let mut renamed: Vec<Rename> = Vec::new();
    for id in &added {
        let Some(c) = b.get(id) else { continue };
        let h = content_hash(c);
        let Some(candidates) = removed_by_hash.get(&h) else {
            continue;
        };
        if let Some(from) = candidates
            .iter()
            .find(|cand| !consumed_removed.contains(*cand))
        {
            renamed.push(Rename {
                from: from.clone(),
                to: id.clone(),
            });
            consumed_removed.insert(from.clone());
        }
    }

    let to_ids: BTreeSet<&ConceptId> = renamed.iter().map(|r| &r.to).collect();
    let added: Vec<ConceptId> = added
        .iter()
        .filter(|id| !to_ids.contains(id))
        .cloned()
        .collect();
    let removed: Vec<ConceptId> = removed
        .iter()
        .filter(|id| !consumed_removed.contains(id))
        .cloned()
        .collect();

    // Per-concept frontmatter and trust changes for ids present in both bundles.
    let mut frontmatter = Vec::new();
    let mut trust = Vec::new();
    for id in a_ids.intersection(&b_ids) {
        let (Some(ca), Some(cb)) = (a.get(id), b.get(id)) else {
            continue;
        };
        if let Some(fc) = frontmatter_diff(ca, cb) {
            frontmatter.push(fc);
        }
        if let Some(tc) = trust_diff(ca, cb) {
            trust.push(tc);
        }
    }

    // Links broken vs mended, keyed by (source id, raw target as written). A
    // rename of the source concept is not tracked here: the source id differs,
    // so a link from a renamed concept is treated as a separate edge.
    let a_broken: BTreeSet<(ConceptId, String)> = a.broken_links().into_iter().collect();
    let b_broken: BTreeSet<(ConceptId, String)> = b.broken_links().into_iter().collect();
    let mended_links: Vec<(ConceptId, String)> = a_broken.difference(&b_broken).cloned().collect();
    let broken_links: Vec<(ConceptId, String)> = b_broken.difference(&a_broken).cloned().collect();

    BundleDiff {
        added,
        removed,
        renamed,
        frontmatter,
        trust,
        mended_links,
        broken_links,
    }
}

/// A best-effort content hash for a concept: the body plus the `type`,
/// `title`, and `description` frontmatter fields.
///
/// These are the identifying fields that do not change when a concept is moved
/// to a new id within the bundle, so equal hashes are a strong signal of a
/// rename. Fields such as `generated` or `verified` are deliberately excluded:
/// a producer may refresh those without touching the content.
fn content_hash(concept: &Concept) -> u64 {
    let mut hasher = DefaultHasher::new();
    concept.document.body.hash(&mut hasher);
    hash_option(&mut hasher, concept.type_().as_deref());
    hash_option(&mut hasher, concept.document.frontmatter.title().as_deref());
    hash_option(
        &mut hasher,
        concept.document.frontmatter.description().as_deref(),
    );
    hasher.finish()
}

/// Hashes an optional value with a presence marker, so `None` and `Some("")`
/// do not collide.
fn hash_option<T: Hash + ?Sized>(hasher: &mut DefaultHasher, opt: Option<&T>) {
    match opt {
        Some(value) => {
            1u8.hash(hasher);
            value.hash(hasher);
        }
        None => 0u8.hash(hasher),
    }
}

/// Computes the frontmatter key changes between two concepts sharing an id.
/// Returns `None` when nothing changed.
fn frontmatter_diff(a: &Concept, b: &Concept) -> Option<FrontmatterChange> {
    let ma = a.document.frontmatter.as_mapping();
    let mb = b.document.frontmatter.as_mapping();
    let keys_a: BTreeSet<String> = ma.keys().map(String::from).collect();
    let keys_b: BTreeSet<String> = mb.keys().map(String::from).collect();

    let added: Vec<String> = keys_b.difference(&keys_a).cloned().collect();
    let removed: Vec<String> = keys_a.difference(&keys_b).cloned().collect();

    let mut changed: Vec<(String, String, String)> = Vec::new();
    for key in keys_a.intersection(&keys_b) {
        let va = ma.get(key).expect("key present in a");
        let vb = mb.get(key).expect("key present in b");
        if va != vb {
            changed.push((key.clone(), scalar(va), scalar(vb)));
        }
    }

    if added.is_empty() && removed.is_empty() && changed.is_empty() {
        None
    } else {
        Some(FrontmatterChange {
            id: a.id.clone(),
            added,
            removed,
            changed,
        })
    }
}

/// Computes the trust tier and status changes between two concepts sharing an
/// id. Returns `None` when neither changed.
fn trust_diff(a: &Concept, b: &Concept) -> Option<TrustChange> {
    let tier = (a.trust_tier(), b.trust_tier());
    let status = (a.status(), b.status());
    let tier = (tier.0 != tier.1).then_some(tier);
    let status = (status.0 != status.1).then_some(status);
    if tier.is_none() && status.is_none() {
        None
    } else {
        Some(TrustChange {
            id: a.id.clone(),
            tier,
            status,
        })
    }
}

/// A scalar's text in display form: the YAML value with its trailing newline
/// trimmed and any internal line breaks collapsed to single spaces. Keeping
/// each changed value on one line preserves the `key: old -> new` layout when
/// a value is a nested mapping or sequence, which `to_yaml_string` would
/// otherwise emit across several lines.
fn scalar(value: &Value) -> String {
    value
        .to_yaml_string()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

impl std::fmt::Display for BundleDiff {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.is_empty() {
            return writeln!(f, "no changes");
        }

        if !self.added.is_empty() {
            writeln!(f, "added ({}):", self.added.len())?;
            for id in &self.added {
                writeln!(f, "  + {id}")?;
            }
        }
        if !self.removed.is_empty() {
            writeln!(f, "removed ({}):", self.removed.len())?;
            for id in &self.removed {
                writeln!(f, "  - {id}")?;
            }
        }
        if !self.renamed.is_empty() {
            writeln!(f, "renamed ({}):", self.renamed.len())?;
            for r in &self.renamed {
                writeln!(f, "  ~ {} -> {}", r.from, r.to)?;
            }
        }
        if !self.frontmatter.is_empty() {
            writeln!(f, "frontmatter ({}):", self.frontmatter.len())?;
            for fc in &self.frontmatter {
                writeln!(f, "  {}:", fc.id)?;
                for k in &fc.added {
                    writeln!(f, "    + {k}")?;
                }
                for k in &fc.removed {
                    writeln!(f, "    - {k}")?;
                }
                for (k, old, new) in &fc.changed {
                    writeln!(f, "    ~ {k}: {old} -> {new}")?;
                }
            }
        }
        if !self.trust.is_empty() {
            writeln!(f, "trust ({}):", self.trust.len())?;
            for tc in &self.trust {
                write!(f, "  {}:", tc.id)?;
                if let Some((from, to)) = &tc.tier {
                    write!(f, " tier {from} -> {to}")?;
                }
                if let Some((from, to)) = &tc.status {
                    write!(f, " status {from} -> {to}")?;
                }
                writeln!(f)?;
            }
        }
        if !self.mended_links.is_empty() {
            writeln!(f, "mended links ({}):", self.mended_links.len())?;
            for (id, target) in &self.mended_links {
                writeln!(f, "  + {id} -> {target}")?;
            }
        }
        if !self.broken_links.is_empty() {
            writeln!(f, "broken links ({}):", self.broken_links.len())?;
            for (id, target) in &self.broken_links {
                writeln!(f, "  - {id} -> {target}")?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::yaml::Value;

    #[test]
    fn hash_option_distinguishes_none_and_empty() {
        let mut with_none = DefaultHasher::new();
        hash_option::<str>(&mut with_none, None);
        let mut with_empty = DefaultHasher::new();
        hash_option(&mut with_empty, Some(""));
        assert_ne!(with_none.finish(), with_empty.finish());

        let mut with_value = DefaultHasher::new();
        hash_option(&mut with_value, Some("revenue"));
        assert_ne!(with_empty.finish(), with_value.finish());
    }

    #[test]
    fn scalar_trims_trailing_newline() {
        assert_eq!(scalar(&Value::String("x".into())), "x");
        assert_eq!(scalar(&Value::Int(7)), "7");
    }
}
