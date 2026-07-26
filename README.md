# okf

A **pure-Rust, zero-dependency** implementation of the [Open Knowledge Format
(OKF) v0.2](https://github.com/GoogleCloudPlatform/knowledge-catalog/blob/main/okf/SPEC.md),
Google's open, human- and agent-friendly format for representing *knowledge* as
a directory of markdown files with YAML frontmatter.

> OKF is intentionally minimal: "if you can `cat` a file, you can read OKF; if
> you can `git clone` a repo, you can ship it." This crate honors that spirit:
> it is implemented entirely on the Rust **standard library**, with **no
> third-party dependencies** (it includes its own YAML-subset parser, markdown
> link scanner, date arithmetic, directory walker, and CLI argument parsing).

## What OKF is

- A **bundle** is a directory tree of UTF-8 markdown files (the unit of
  distribution).
- A **concept** is one markdown document: a YAML **frontmatter** block delimited
  by `---`, followed by a markdown **body**.
- A **concept id** is the file's path within the bundle with `.md` removed
  (`tables/users.md` becomes `tables/users`). The spec constrains no character
  in a filename, so ids may contain spaces, emoji, and other Unicode; an id
  outside the conventional `[A-Za-z0-9_][A-Za-z0-9_.-]*` set is reported as a
  warning, never rejected.
- Concepts **cross-link** via ordinary markdown links, absolute
  (`/tables/users.md`, bundle-relative) or relative (`./other.md`). A target
  containing a space may also be written `<...>` or percent-encoded.
- `index.md` files provide directory listings for *progressive disclosure*;
  `log.md` files record date-grouped change history. Both are **reserved**
  filenames.
- The only hard requirement for **conformance** is a non-empty `type` field on
  every concept; consumers must otherwise be permissive (unknown types, unknown
  keys, broken links, and missing optional fields are all tolerated).

## What v0.2 adds

v0.2 assumes a corpus that is continuously written and maintained by agents, and
makes the questions such a corpus raises answerable from frontmatter. Every new
key is optional, and *absence is meaningful rather than invalid*, so a v0.1
document is still a conformant v0.2 document.

| Question                                 | Frontmatter                                        | Module          |
|------------------------------------------|----------------------------------------------------|-----------------|
| What was this created from? (provenance) | `sources`, `usage_window` (§5.1)                    | [`provenance`]  |
| How much should I trust it? (trust)      | `generated`, `verified` (§5.2), trust tiers (§5.3)  | [`trust`]       |
| Is it still true? (freshness)            | `stale_after` (§5.5)                                | [`trust`]       |
| Is it the current version? (lifecycle)   | `status` (§5.4)                                     | [`trust`]       |
| Was this number produced the way we said it must be? (attestation) | `runtime`, `parameters`, `computation`, `executor`, `attester` (§10) | [`computation`] |

Plus the actor convention shared by every identity field
(`<producer>/<version>`, `human:<id>`, `process:<id>`, §7) in [`actor`], and
per-claim attribution through markdown footnotes keyed to `sources[].id` (§5.1)
in [`footnotes`].

Two v0.1 constructs are superseded (§13.1) but still readable, since a v0.2
consumer is expected to handle v0.1 bundles:

| v0.1               | v0.2                    | Fallback in this crate                  |
|--------------------|-------------------------|-----------------------------------------|
| `timestamp`        | `generated: { by, at }` | `Frontmatter::content_changed_at`       |
| body `# Citations` | `sources` + footnotes   | `Document::citations` still parses it   |

`okf validate` reports both as warnings so a bundle can be migrated
incrementally, without ever failing conformance for using the old form.

### Attestation is recorded, not executed

An `Attested Computation` concept (§10) carries a sanctioned way to compute a
value: a `runtime`, typed `parameters`, the computation itself (inline under
`# Computation` or in a file), an `executor` that produces a receipt, and a
deterministic `attester` that turns a receipt into a verdict.

This crate models and checks that contract. It **never runs anything**: the
receipt and verdict are runtime artifacts that §10.5 explicitly keeps out of the
bundle. Executing computations and attesting receipts are consumer-side
concerns.

## Library overview

| Module          | Responsibility                                                        |
|-----------------|-----------------------------------------------------------------------|
| [`yaml`]        | A YAML-*subset* `Value`/`Mapping`, parser, and emitter for frontmatter |
| [`document`]    | `Document` = frontmatter + body; parse / serialize / validate (§4)    |
| [`frontmatter`] | `Frontmatter`: typed accessors over an order-preserving mapping (§4.1)|
| [`concept_id`]  | `ConceptId` to/from path conversion and segment rules (§2)            |
| [`provenance`]  | `sources`, credibility signals, and footnote attribution (§5.1)       |
| [`trust`]       | `generated`, `verified`, trust tiers, `status`, `stale_after` (§5.2 to §5.5) |
| [`actor`]       | The `human:` / `process:` / `<producer>/<version>` convention (§7)    |
| [`date`]        | `Date`/`DateTime` parsing and comparison for the date-valued fields   |
| [`computation`] | The Attested Computation contract and its `# Computation` block (§10) |
| [`footnotes`]   | `[^label]` reference and definition scanning (§5.1)                   |
| [`links`]       | Markdown link extraction, classification, path-valued fields (§6)     |
| [`bundle`]      | `Bundle::load`: walk a tree, build the link and derivation graphs (§3, §5.1, §6) |
| [`index`]       | Generate `index.md` directory listings (§8)                           |
| [`log`]         | Parse / build `log.md` update histories (§9)                          |
| [`validate`]    | §11 conformance checking with severity-tagged diagnostics             |

The core split mirrors the reference Python implementation's `bundle/` package
(`document.py`, `index.py`, `paths.py`, `synthesizer.py`) so behaviour stays
compatible: the document parser, validator, and index generator are faithful
ports, verified by tests adapted from the reference test suite. Frontmatter can
also be reordered into the key order the reference writes
(`Frontmatter::reorder_preferred`, `PREFERRED_KEY_ORDER`).

Compatibility is checked against the reference's four published bundles
(`acme_retail`, `crypto_bitcoin`, `ga4`, `stackoverflow`): all 53 concepts load,
every one is conformant, and each document's frontmatter re-serializes to a value
PyYAML reads back identically.

### Design choices

- **Frontmatter preserves everything.** Rather than deserializing into a fixed
  struct (which would drop producer-defined keys), `Frontmatter` keeps the full
  ordered mapping and layers typed getters (`type_()`, `sources()`,
  `trust_tier()`, and so on) on top. This satisfies the spec's requirement that
  consumers preserve unknown keys when round-tripping.
- **Signals are stored, verdicts are derived.** Trust tiers (§5.3) and source
  credibility (§5.1) are computed on read, never stored, because a stored score
  is subjective, unportable across consumers, and goes stale.
- **Staleness is opt-in.** `validate_bundle` is deterministic and never consults
  the clock; `validate_bundle_at(&bundle, today)` adds the `stale_after`
  comparison. The CLI passes the system date, or `--today YYYY-MM-DD`.
- **Permissive loading.** `Bundle::load` never aborts on a bad concept file; it
  collects parse failures in `parse_errors()` and keeps going. Broken
  cross-links are retained as graph edges to non-existent concepts, and a
  malformed date is reported rather than dropped (`DateField` keeps the raw
  scalar alongside its parse).
- **Validation rejects only what §11 rejects.** `Document::validate()` requires a
  non-empty `type` and nothing more, matching the reference implementation.
  Everything else the spec asks of a producer is reported, never enforced:
  `Document::missing_recommended()` returns the unset recommended keys
  (`title`, `description`, `generated`, plus `runtime` on an Attested
  Computation), and `validate_bundle` surfaces them as warnings.
- **A documented YAML subset.** Real OKF frontmatter is scalars, lists, and
  shallow maps. The parser handles block/flow collections, quoted/plain
  scalars, `|`/`>` block scalars, and comments; it rejects (with a clear error)
  the YAML features that never appear in frontmatter: anchors, tags, multiple
  documents. Colons inside flow scalars are content, not separators, so
  `{ by: human:ahormati, at: 2026-06-25T09:00:00Z }` parses as v0.2 intends.
  Scalars may also span lines, folding each break into a space, because PyYAML
  wraps any value past 80 columns and the reference publishes bundles that way.
- **Timestamps stay strings.** YAML's implicit resolver would type a bare
  `2026-06-30T14:00:00Z` as a datetime; this crate keeps it as text with the
  parse alongside (`DateTimeField`), so a malformed date can be *reported*
  rather than silently dropped. On the way out a datetime-valued scalar is
  emitted quoted, because a bare one is not stable even under the reference's own
  round-trip: PyYAML re-dumps it as `2026-06-30 14:00:00+00:00`, losing the `T`
  and `Z` that §5.2 asks for. A bare `YYYY-MM-DD` stays plain.

## Usage

### As a library

```rust,no_run
use okf::{Bundle, validate_bundle, ConceptId, Date, TrustTier};

let bundle = Bundle::load("./my_bundle")?;
println!("{} concepts", bundle.len());

// Conformance check (§11).
let report = validate_bundle(&bundle);
if report.is_conformant() {
    println!("conformant with OKF v{}", okf::OKF_VERSION);
}

// Traverse the cross-link graph.
let id = ConceptId::parse("tables/orders")?;
for link in bundle.links_from(&id) {
    println!("{} -> {} (exists: {})", id, link.target, link.exists);
}
for backlink in bundle.backlinks(&id) {
    println!("cited by {backlink}");
}

// Trust and freshness (§5).
let today = Date::today_utc().unwrap();
for concept in bundle.concepts() {
    if concept.trust_tier() < TrustTier::HumanReviewed && concept.is_stale_on(today) {
        println!("{} needs review", concept.id);
    }
}

// Provenance: recurse into sources that are themselves concepts (§5.1).
for source in bundle.derived_from(&id) {
    println!("{id} derives from {source}");
}
# Ok::<(), Box<dyn std::error::Error>>(())
```

Reading an Attested Computation contract:

```rust
use okf::Document;

let doc = Document::parse(
    "---\n\
     type: Attested Computation\n\
     runtime: bigquery\n\
     parameters:\n\
     \x20 - { name: year, type: integer, required: true }\n\
     executor:\n\
     \x20 resource: references/skills/run-on-bq.md\n\
     \x20 receipt: [job_id, executed_sql, result]\n\
     ---\n\n# Computation\n\n\
     \x20   SELECT SUM(amount) FROM finance.recognized_revenue WHERE fiscal_year = @year\n",
)?;

let contract = doc.attested_computation().unwrap();
assert_eq!(contract.runtime.as_deref(), Some("bigquery"));
assert_eq!(contract.required_parameters().count(), 1);
assert!(contract.computation.code().unwrap().starts_with("SELECT SUM(amount)"));
# Ok::<(), okf::DocumentError>(())
```

### As a CLI

```text
okf validate     <bundle>    Check a bundle against OKF v0.2 conformance (§11)
okf info         <bundle>    Summarize a bundle (concepts, types, trust, links)
okf trust        <bundle>    Report trust tier, status, and staleness per concept
okf computations <bundle>    List Attested Computation contracts (§10)
okf index        <bundle>    (Re)generate every index.md in the bundle
okf graph        <bundle>    Print the cross-link graph (--dot for Graphviz DOT)
okf parse        <file>      Parse one concept document and print its structure
okf fmt          <file>      Normalize a document by parse + re-serialize (-w writes)
```

`okf validate` exits non-zero when a bundle is not conformant, so it drops
straight into CI:

```sh
okf validate ./bundles/finance
okf validate ./bundles/finance --today 2026-07-01   # pin staleness for reproducible runs
okf graph ./bundles/finance --dot --sources | dot -Tsvg > graph.svg
```

`okf trust` gives the per-concept view the trust families exist for:

```text
computations/profit [stable] machine-confirmed STALE
  generated: reference_agent/gemini-2.5-pro at 2026-06-14T14:00:00Z
  verified:  process:finance-nightly at 2026-06-12T08:00:00Z
  stale_after: 2026-06-15
  source:    [cost-alloc] Cost allocation standard
computations/revenue [stable] human-reviewed
  generated: reference_agent/gemini-2.5-pro at 2026-06-28T14:00:00Z
  verified:  human:ahormati at 2026-06-25T09:00:00Z
  stale_after: 2026-12-31
```

## Mapping to the spec

| Spec section                | Implemented by                                            |
|-----------------------------|-----------------------------------------------------------|
| §2 Terminology / concept id | [`concept_id::ConceptId`]                                 |
| §3 Bundle structure         | [`bundle::Bundle`], [`bundle::RESERVED_FILENAMES`]        |
| §4 Concept documents        | [`document::Document`], [`frontmatter::Frontmatter`]      |
| §5.1 Provenance             | [`provenance::Source`], [`provenance::attributions`]      |
| §5.2 Trust                  | [`trust::Generated`], [`trust::Verification`]             |
| §5.3 Trust tiers            | [`trust::TrustTier`]                                      |
| §5.4 / §5.5 Lifecycle       | [`trust::Status`], [`trust::is_stale_on`]                 |
| §6 Cross-linking and paths  | [`links`], [`links::field_path_candidates`]               |
| §7 Actor convention         | [`actor::Actor`]                                          |
| §8 Index files              | [`index::regenerate_indexes`]                             |
| §9 Log files                | [`log::Log`]                                              |
| §10 Attested computations   | [`computation::AttestedComputation`]                      |
| §11 Conformance             | [`validate::validate_bundle`]                             |
| §12 Versioning              | [`bundle::Bundle::okf_version`], [`OKF_VERSION`]          |
| §13 Changes from v0.1       | [`frontmatter::LEGACY_FRONTMATTER_KEYS`]                  |

## Building & testing

```sh
cargo build            # library + `okf` binary
cargo test             # unit + integration tests (incl. ports of the reference tests)
cargo clippy --all-targets
```

## License

Licensed under the **Apache License, Version 2.0**, the same license as the
upstream [OKF project](https://github.com/GoogleCloudPlatform/knowledge-catalog/tree/main/okf).
This crate is a derivative work: its document parser, concept-id conventions,
and index generator are ports of the OKF reference implementation. See
[`LICENSE`](LICENSE) for the full terms and [`NOTICE`](NOTICE) for attribution.

This is an independent implementation and is not affiliated with or endorsed by
Google.

[`yaml`]: https://docs.rs/okf/latest/okf/yaml/
[`document`]: https://docs.rs/okf/latest/okf/document/
[`frontmatter`]: https://docs.rs/okf/latest/okf/frontmatter/
[`concept_id`]: https://docs.rs/okf/latest/okf/concept_id/
[`provenance`]: https://docs.rs/okf/latest/okf/provenance/
[`trust`]: https://docs.rs/okf/latest/okf/trust/
[`actor`]: https://docs.rs/okf/latest/okf/actor/
[`date`]: https://docs.rs/okf/latest/okf/date/
[`computation`]: https://docs.rs/okf/latest/okf/computation/
[`footnotes`]: https://docs.rs/okf/latest/okf/footnotes/
[`links`]: https://docs.rs/okf/latest/okf/links/
[`bundle`]: https://docs.rs/okf/latest/okf/bundle/
[`index`]: https://docs.rs/okf/latest/okf/index/
[`log`]: https://docs.rs/okf/latest/okf/log/
[`validate`]: https://docs.rs/okf/latest/okf/validate/
[`concept_id::ConceptId`]: https://docs.rs/okf/latest/okf/concept_id/struct.ConceptId.html
[`bundle::Bundle`]: https://docs.rs/okf/latest/okf/bundle/struct.Bundle.html
[`bundle::Bundle::okf_version`]: https://docs.rs/okf/latest/okf/bundle/struct.Bundle.html#method.okf_version
[`bundle::RESERVED_FILENAMES`]: https://docs.rs/okf/latest/okf/bundle/constant.RESERVED_FILENAMES.html
[`document::Document`]: https://docs.rs/okf/latest/okf/document/struct.Document.html
[`frontmatter::Frontmatter`]: https://docs.rs/okf/latest/okf/frontmatter/struct.Frontmatter.html
[`frontmatter::LEGACY_FRONTMATTER_KEYS`]: https://docs.rs/okf/latest/okf/frontmatter/constant.LEGACY_FRONTMATTER_KEYS.html
[`provenance::Source`]: https://docs.rs/okf/latest/okf/provenance/struct.Source.html
[`provenance::attributions`]: https://docs.rs/okf/latest/okf/provenance/fn.attributions.html
[`trust::Generated`]: https://docs.rs/okf/latest/okf/trust/struct.Generated.html
[`trust::Verification`]: https://docs.rs/okf/latest/okf/trust/struct.Verification.html
[`trust::TrustTier`]: https://docs.rs/okf/latest/okf/trust/enum.TrustTier.html
[`trust::Status`]: https://docs.rs/okf/latest/okf/trust/enum.Status.html
[`trust::is_stale_on`]: https://docs.rs/okf/latest/okf/trust/fn.is_stale_on.html
[`links::field_path_candidates`]: https://docs.rs/okf/latest/okf/links/fn.field_path_candidates.html
[`actor::Actor`]: https://docs.rs/okf/latest/okf/actor/struct.Actor.html
[`index::regenerate_indexes`]: https://docs.rs/okf/latest/okf/index/fn.regenerate_indexes.html
[`log::Log`]: https://docs.rs/okf/latest/okf/log/struct.Log.html
[`computation::AttestedComputation`]: https://docs.rs/okf/latest/okf/computation/struct.AttestedComputation.html
[`validate::validate_bundle`]: https://docs.rs/okf/latest/okf/validate/fn.validate_bundle.html
[`OKF_VERSION`]: https://docs.rs/okf/latest/okf/constant.OKF_VERSION.html
