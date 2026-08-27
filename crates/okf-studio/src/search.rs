//! Fuzzy matching, the omnisearch index, and the shared query syntax.
//!
//! One scorer serves omnisearch, the palette's command mode, refactor form
//! completion, tree type-ahead, and graph filtering. The syntax layer adds
//! the cheap, composable filters (`#tag`, `type:Policy`, `tier:unverified`,
//! `is:stale`, `is:broken`) reused by every filterable view.

use okf_core::{ConceptId, Status, TrustTier};
use std::str::FromStr;

/// One concept's searchable representation, precomputed at snapshot build.
#[derive(Clone, Debug)]
pub struct SearchEntry {
    /// The concept id.
    pub id: ConceptId,
    /// Display title.
    pub title: String,
    /// One-line description (may be empty).
    pub description: String,
    /// Frontmatter tags.
    pub tags: Vec<String>,
    /// Body headings, for heading-level hits.
    pub headings: Vec<String>,
    /// The concept `type`.
    pub type_: String,
    /// Trust tier, for `tier:` filters.
    pub tier: TrustTier,
    /// Lifecycle status, for `status:` filters.
    pub status: Status,
    /// Whether the concept is stale today, for `is:stale`.
    pub stale: bool,
    /// Whether the concept has broken outgoing links, for `is:broken`.
    pub broken: bool,
}

/// The precomputed omnisearch index over a snapshot's concepts.
#[derive(Clone, Debug, Default)]
pub struct SearchIndex {
    /// One entry per concept, in bundle order.
    pub entries: Vec<SearchEntry>,
}

/// A single omnisearch result.
#[derive(Clone, Debug)]
pub struct SearchHit {
    /// The concept the hit points at.
    pub id: ConceptId,
    /// The heading within the concept, when the hit is heading-level.
    pub heading: Option<String>,
    /// Fuzzy score (higher is better).
    pub score: i32,
    /// Char indices of the query match within [`SearchHit::label`].
    pub indices: Vec<usize>,
    /// The text the match was scored against.
    pub label: String,
}

/// A structured filter parsed from the shared query syntax.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Filter {
    /// `#tag`
    Tag(String),
    /// `type:Policy`
    Type(String),
    /// `tier:unverified`
    Tier(TrustTier),
    /// `status:draft`
    Status(String),
    /// `is:stale`
    Stale,
    /// `is:broken`
    Broken,
}

/// A parsed query: free text plus zero or more filters.
#[derive(Clone, Debug, Default)]
pub struct Query {
    /// The fuzzy free-text part.
    pub text: String,
    /// The structured filters.
    pub filters: Vec<Filter>,
}

impl Query {
    /// Parses the shared query syntax: whitespace-separated terms, where
    /// `#x`, `type:x`, `tier:x`, `status:x`, `is:stale`, and `is:broken`
    /// become filters and everything else joins the fuzzy text.
    #[must_use]
    pub fn parse(raw: &str) -> Self {
        let mut text_terms: Vec<&str> = Vec::new();
        let mut filters = Vec::new();
        for term in raw.split_whitespace() {
            if let Some(tag) = term.strip_prefix('#') {
                if !tag.is_empty() {
                    filters.push(Filter::Tag(tag.to_string()));
                    continue;
                }
            } else if let Some(t) = term.strip_prefix("type:") {
                filters.push(Filter::Type(t.to_string()));
                continue;
            } else if let Some(t) = term.strip_prefix("tier:") {
                if let Ok(tier) = TrustTier::from_str(t) {
                    filters.push(Filter::Tier(tier));
                    continue;
                }
            } else if let Some(s) = term.strip_prefix("status:") {
                filters.push(Filter::Status(s.to_string()));
                continue;
            } else if term == "is:stale" {
                filters.push(Filter::Stale);
                continue;
            } else if term == "is:broken" {
                filters.push(Filter::Broken);
                continue;
            }
            text_terms.push(term);
        }
        Self {
            text: text_terms.join(" "),
            filters,
        }
    }

    /// Whether an entry passes every filter.
    #[must_use]
    pub fn matches_filters(&self, entry: &SearchEntry) -> bool {
        self.filters.iter().all(|f| match f {
            Filter::Tag(tag) => entry.tags.iter().any(|t| t.eq_ignore_ascii_case(tag)),
            Filter::Type(t) => entry.type_.eq_ignore_ascii_case(t),
            Filter::Tier(tier) => entry.tier == *tier,
            Filter::Status(s) => entry.status.as_str().eq_ignore_ascii_case(s),
            Filter::Stale => entry.stale,
            Filter::Broken => entry.broken,
        })
    }
}

impl SearchIndex {
    /// Runs a query over the index, returning at most `limit` hits, best
    /// first. An empty free-text query returns every entry passing the
    /// filters, in index order.
    #[must_use]
    pub fn search(&self, raw_query: &str, limit: usize) -> Vec<SearchHit> {
        let query = Query::parse(raw_query);
        let mut hits: Vec<SearchHit> = Vec::new();
        for entry in &self.entries {
            if !query.matches_filters(entry) {
                continue;
            }
            if query.text.is_empty() {
                hits.push(SearchHit {
                    id: entry.id.clone(),
                    heading: None,
                    score: 0,
                    indices: Vec::new(),
                    label: entry.id.to_string(),
                });
                continue;
            }
            // Concept-level hit: best score across id, title, description,
            // and tags.
            let id_str = entry.id.to_string();
            let mut best: Option<SearchHit> = None;
            let candidates: Vec<&str> = std::iter::once(id_str.as_str())
                .chain(std::iter::once(entry.title.as_str()))
                .chain(std::iter::once(entry.description.as_str()))
                .chain(entry.tags.iter().map(String::as_str))
                .collect();
            for hay in candidates {
                if let Some((score, indices)) = fuzzy_match(&query.text, hay)
                    && best.as_ref().is_none_or(|b| score > b.score)
                {
                    best = Some(SearchHit {
                        id: entry.id.clone(),
                        heading: None,
                        score,
                        indices,
                        label: hay.to_string(),
                    });
                }
            }
            if let Some(hit) = best {
                hits.push(hit);
            }
            // Heading-level hits are separate result rows.
            for heading in &entry.headings {
                if let Some((score, indices)) = fuzzy_match(&query.text, heading) {
                    hits.push(SearchHit {
                        id: entry.id.clone(),
                        heading: Some(heading.clone()),
                        // Slightly discounted so the concept row leads.
                        score: score - 1,
                        indices,
                        label: heading.clone(),
                    });
                }
            }
        }
        hits.sort_by(|a, b| b.score.cmp(&a.score).then_with(|| a.id.cmp(&b.id)));
        hits.truncate(limit);
        hits
    }
}

const BONUS_BOUNDARY: i32 = 16;
const BONUS_CAMEL: i32 = 12;
const BONUS_CONSECUTIVE: i32 = 8;
const BONUS_FIRST_CHAR: i32 = 20;
const PENALTY_GAP_START: i32 = -3;
const PENALTY_GAP_EXTEND: i32 = -1;
const MATCH_SCORE: i32 = 16;

/// Scores `query` against `haystack` with a Smith-Waterman-style alignment.
///
/// Subsequence match with affine gap penalties, plus bonuses at the start of
/// the haystack, after `/ _ - . :` separators and whitespace, and at
/// camelCase boundaries — tuned so `pte` finds `policies/travel_expenses`
/// via segment initials.
///
/// Case-insensitive by default; a query char written in uppercase must match
/// exactly (smart-case). Returns the score and the matched char indices, or
/// `None` when `query` is not a subsequence of `haystack`.
#[must_use]
pub fn fuzzy_match(query: &str, haystack: &str) -> Option<(i32, Vec<usize>)> {
    const NEG: i32 = i32::MIN / 4;

    let query_chars: Vec<char> = query.chars().filter(|c| !c.is_whitespace()).collect();
    let haystack_chars: Vec<char> = haystack.chars().collect();
    if query_chars.is_empty() {
        return Some((0, Vec::new()));
    }
    if query_chars.len() > haystack_chars.len() {
        return None;
    }

    let eq = |qc: char, hc: char| {
        if qc.is_uppercase() {
            qc == hc
        } else {
            qc.to_lowercase().eq(hc.to_lowercase())
        }
    };
    let bonus_at = |idx: usize| -> i32 {
        if idx == 0 {
            return BONUS_FIRST_CHAR;
        }
        let prev = haystack_chars[idx - 1];
        if matches!(prev, '/' | '_' | '-' | '.' | ':') || prev.is_whitespace() {
            BONUS_BOUNDARY
        } else if prev.is_lowercase() && haystack_chars[idx].is_uppercase() {
            BONUS_CAMEL
        } else {
            0
        }
    };

    let (q_len, h_len) = (query_chars.len(), haystack_chars.len());
    // dp[i][j]: best score with query_chars[i] aligned at haystack_chars[j]; parent[i][j] is the
    // position of query_chars[i-1] on that best path.
    let mut dp = vec![vec![NEG; h_len]; q_len];
    let mut parent = vec![vec![usize::MAX; h_len]; q_len];

    for (j, &hc) in haystack_chars.iter().enumerate() {
        if eq(query_chars[0], hc) {
            // A leading gap is free: matching later in the haystack is not
            // penalized, only rewarded less when it lacks a boundary bonus.
            dp[0][j] = MATCH_SCORE + bonus_at(j);
        }
    }
    for i in 1..q_len {
        // best_prev = max over k <= j-2 of dp[i-1][k] plus the affine gap
        // cost of the cells between k and the current j; every candidate
        // decays at the same rate, so a running max suffices.
        let mut best_prev = NEG;
        let mut best_prev_j = usize::MAX;
        for j in i..h_len {
            if best_prev > NEG {
                best_prev += PENALTY_GAP_EXTEND;
            }
            if j >= 2 && dp[i - 1][j - 2] > NEG {
                let candidate = dp[i - 1][j - 2] + PENALTY_GAP_START;
                if candidate > best_prev {
                    best_prev = candidate;
                    best_prev_j = j - 2;
                }
            }
            if !eq(query_chars[i], haystack_chars[j]) {
                continue;
            }
            let consecutive = if dp[i - 1][j - 1] > NEG {
                dp[i - 1][j - 1] + MATCH_SCORE + BONUS_CONSECUTIVE + bonus_at(j)
            } else {
                NEG
            };
            let gapped = if best_prev > NEG {
                best_prev + MATCH_SCORE + bonus_at(j)
            } else {
                NEG
            };
            if consecutive >= gapped {
                if consecutive > NEG {
                    dp[i][j] = consecutive;
                    parent[i][j] = j - 1;
                }
            } else {
                dp[i][j] = gapped;
                parent[i][j] = best_prev_j;
            }
        }
    }

    let (mut best_j, mut best_score) = (usize::MAX, NEG);
    for (j, &score) in dp[q_len - 1].iter().enumerate() {
        if score > best_score {
            best_score = score;
            best_j = j;
        }
    }
    if best_j == usize::MAX {
        return None;
    }
    let mut indices = vec![0usize; q_len];
    let mut cursor = best_j;
    for i in (0..q_len).rev() {
        indices[i] = cursor;
        if i > 0 {
            cursor = parent[i][cursor];
        }
    }
    Some((best_score, indices))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_segment_initials() {
        let (score, idx) = fuzzy_match("pte", "policies/travel_expenses").unwrap();
        assert!(score > 0);
        assert_eq!(idx, vec![0, 9, 16]);
    }

    #[test]
    fn prefers_boundary_matches() {
        let (loose, _) = fuzzy_match("te", "notes").unwrap();
        let (boundary, _) = fuzzy_match("te", "travel_expenses").unwrap();
        assert!(boundary > loose);
    }

    #[test]
    fn non_subsequence_is_none() {
        assert!(fuzzy_match("xyz", "policies").is_none());
        assert!(fuzzy_match("aa", "a").is_none());
    }

    #[test]
    fn smart_case() {
        assert!(fuzzy_match("Pol", "policies").is_none());
        assert!(fuzzy_match("pol", "Policies").is_some());
    }

    #[test]
    fn query_syntax_parses_filters() {
        let q = Query::parse("trav #hr type:Policy tier:unverified is:stale is:broken");
        assert_eq!(q.text, "trav");
        assert_eq!(q.filters.len(), 5);
        assert!(q.filters.contains(&Filter::Tag("hr".into())));
        assert!(q.filters.contains(&Filter::Type("Policy".into())));
        assert!(q.filters.contains(&Filter::Tier(TrustTier::Unverified)));
        assert!(q.filters.contains(&Filter::Stale));
        assert!(q.filters.contains(&Filter::Broken));
    }
}
