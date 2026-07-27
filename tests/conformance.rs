//! End-to-end conformance tests for [`validate_bundle`] and the §11 clauses,
//! driven by the specification's Appendix A worked example: an income statement
//! whose two figures are split into attested computations, with every
//! frontmatter family populated and the two computations deliberately left in
//! different trust and freshness states.
//!
//! The documents below are transcribed from the spec, so these tests double as
//! a fidelity check: if the crate cannot read the spec's own examples, it does
//! not implement the spec. The reference implementation has no counterpart to
//! this file, since it validates a document at a time rather than a bundle.

mod common;

use common::TempDir;
use okf::{
    validate_bundle, validate_bundle_at, ActorKind, Bundle, ComputationSource, ConceptId, Date,
    Document, ResourceKind, Severity, Status, TrustTier,
};

const INCOME_STATEMENT: &str = r"---
type: Metric
title: Income statement (fiscal year)
description: Headline income-statement figures for a fiscal year.
tags: [finance, income-statement]
status: stable
generated: { by: reference_agent/gemini-2.5-pro, at: 2026-06-20T22:53:05Z }
verified: { by: human:ahormati, at: 2026-06-25T09:00:00Z }
stale_after: 2026-12-31
sources:
  - id: fpa-handbook
    resource: https://wiki.acme/finance/fpa-handbook
    title: FP&A reporting handbook
---

# Definition
The income statement reports [revenue](../computations/revenue.md) and
[gross profit](../computations/profit.md) for a fiscal year, per the FP&A
reporting handbook.[^fpa-handbook] Each figure is produced by a sanctioned,
attestable computation; this concept only narrates them.

[^fpa-handbook]: FP&A reporting handbook
";

const REVENUE: &str = r"---
type: Attested Computation
title: Revenue for fiscal year
description: Recognized revenue for a fiscal year, per Finance's definition.
tags: [finance, revenue]
status: stable
runtime: bigquery
parameters:
  - { name: year, type: integer, required: true }
executor:
  resource: references/skills/run-on-bq.md
  receipt: [job_id, executed_sql, result]
attester:
  resource: references/attesters/sql-equality.py
generated: { by: reference_agent/gemini-2.5-pro, at: 2026-06-28T14:00:00Z }
verified: { by: human:ahormati, at: 2026-06-25T09:00:00Z }
stale_after: 2026-12-31
sources:
  - id: rev-policy
    resource: https://wiki.acme/finance/revenue-recognition
    title: Revenue recognition policy
    author: team:finance-fpa
    last_modified: 2026-04-02
  - id: exec-rev-dash
    resource: dashboards/exec-revenue
    title: Executive revenue dashboard
    author: team:finance-fpa
    usage_count: 5000
    last_modified: 2026-06-18
usage_window: { from: 2026-06-01, to: 2026-06-30 }
---

# Computation

    SELECT SUM(amount) AS revenue
    FROM finance.recognized_revenue
    WHERE fiscal_year = @year

Recognized revenue per the recognition policy,[^rev-policy] corroborated by
the executive revenue dashboard.[^exec-rev-dash]

[^rev-policy]: Revenue recognition policy
[^exec-rev-dash]: Executive revenue dashboard
";

const PROFIT: &str = r"---
type: Attested Computation
title: Gross profit for fiscal year
description: Gross profit by segment for a fiscal year, per the cost-allocation standard.
tags: [finance, profit]
status: stable
runtime: dbt
parameters:
  - { name: year, type: integer, required: true }
  - { name: segment, type: string, required: true }
executor:
  resource: references/skills/run-dbt.md
  receipt: [run_id, compiled_sql, result]
attester:
  resource: references/attesters/dbt-binding.py
generated: { by: reference_agent/gemini-2.5-pro, at: 2026-06-14T14:00:00Z }
verified: { by: process:finance-nightly, at: 2026-06-12T08:00:00Z }
stale_after: 2026-06-15
sources:
  - id: cost-alloc
    resource: https://wiki.acme/finance/cost-allocation
    title: Cost allocation standard
---

# Computation

    SELECT gross_profit
    FROM {{ ref('fct_income_statement') }}
    WHERE fiscal_year = {{ var('year') }}
      AND segment = {{ var('segment') }}

Gross profit by segment per the cost-allocation standard.[^cost-alloc]

[^cost-alloc]: Cost allocation standard
";

/// The Appendix A bundle, `bundles/finance/`.
fn finance_bundle() -> TempDir {
    let tmp = TempDir::new();
    tmp.write("index.md", "---\nokf_version: \"0.2\"\n---\n\n# Finance\n");
    tmp.write("metrics/income-statement.md", INCOME_STATEMENT);
    tmp.write("computations/revenue.md", REVENUE);
    tmp.write("computations/profit.md", PROFIT);
    tmp.write(
        "references/skills/run-on-bq.md",
        "---\ntype: Reference\ntitle: Run on BigQuery\n---\n\nRun instructions.\n",
    );
    tmp.write(
        "references/skills/run-dbt.md",
        "---\ntype: Reference\ntitle: Run dbt\n---\n\nRun instructions.\n",
    );
    tmp.write(
        "references/attesters/sql-equality.py",
        "# deterministic check\n",
    );
    tmp.write(
        "references/attesters/dbt-binding.py",
        "# deterministic check\n",
    );
    tmp
}

fn id(s: &str) -> ConceptId {
    ConceptId::parse(s).unwrap()
}

#[test]
fn appendix_a_bundle_loads_and_conforms() {
    let tmp = finance_bundle();
    let bundle = Bundle::load(tmp.path()).unwrap();

    // The two `.py` attesters are not markdown, so they are not concepts.
    assert_eq!(bundle.len(), 5);
    assert!(bundle.parse_errors().is_empty());
    assert_eq!(bundle.okf_version().as_deref(), Some("0.2"));

    let report = validate_bundle(&bundle);
    assert!(report.is_conformant(), "{:#?}", report.diagnostics);
    assert_eq!(report.error_count(), 0);
}

#[test]
fn narrative_concept_links_to_each_computation() {
    let tmp = finance_bundle();
    let bundle = Bundle::load(tmp.path()).unwrap();

    let statement = id("metrics/income-statement");
    let targets: Vec<String> = bundle
        .links_from(&statement)
        .iter()
        .map(|l| l.target.to_string())
        .collect();
    assert_eq!(targets, vec!["computations/revenue", "computations/profit"]);
    assert!(bundle.links_from(&statement).iter().all(|l| l.exists));

    // Each computation knows which concepts use it (§10.5's discovery path).
    assert_eq!(bundle.backlinks(&id("computations/revenue")), &[statement]);
}

#[test]
fn trust_tiers_and_freshness_differ_per_computation() {
    let tmp = finance_bundle();
    let bundle = Bundle::load(tmp.path()).unwrap();

    let revenue = bundle.get(&id("computations/revenue")).unwrap();
    let profit = bundle.get(&id("computations/profit")).unwrap();

    // A human signed off revenue; only a process signed off profit (§5.3).
    assert_eq!(revenue.trust_tier(), TrustTier::HumanReviewed);
    assert_eq!(profit.trust_tier(), TrustTier::MachineConfirmed);
    assert!(revenue.trust_tier() > profit.trust_tier());
    assert_eq!(revenue.status(), Status::Stable);

    // "Revenue can be fresh while profit is past its stale_after" (§10.4).
    let today = Date::new(2026, 7, 1).unwrap();
    assert!(!revenue.is_stale_on(today));
    assert!(profit.is_stale_on(today));
    let stale = bundle.stale_on(today);
    assert_eq!(stale.len(), 1);
    assert_eq!(stale[0].id, id("computations/profit"));

    // A bare `verified` mapping is one event, not a parse failure (§5.2).
    let verified = revenue.document.frontmatter.verified();
    assert_eq!(verified.len(), 1);
    assert_eq!(verified[0].by.as_ref().unwrap().kind(), ActorKind::Human);

    let generated = revenue.document.frontmatter.generated().unwrap();
    assert_eq!(
        generated.by.as_ref().unwrap().producer(),
        Some("reference_agent")
    );
    // `verified` is independent of `generated.at`: content changed after the
    // human sign-off, and that is legal (§5.2).
    let generated_at = generated.at.unwrap().datetime.unwrap();
    let verified_at = verified[0].at.as_ref().unwrap().datetime.unwrap();
    assert!(generated_at > verified_at);
}

#[test]
fn sources_carry_credibility_signals_framed_by_a_usage_window() {
    let tmp = finance_bundle();
    let bundle = Bundle::load(tmp.path()).unwrap();
    let revenue = bundle.get(&id("computations/revenue")).unwrap();
    let fm = &revenue.document.frontmatter;

    let sources = fm.sources();
    assert_eq!(sources.len(), 2);

    let policy = &sources[0];
    assert_eq!(policy.resource_kind(), ResourceKind::Url);
    assert_eq!(policy.author.as_ref().unwrap().as_str(), "team:finance-fpa");
    assert_eq!(
        policy.last_modified.as_ref().unwrap().date,
        Date::new(2026, 4, 2)
    );
    assert_eq!(policy.usage_count, None);

    // The shared window frames the dashboard's usage_count (§5.1).
    let dashboard = &sources[1];
    assert_eq!(dashboard.usage_count, Some(5000));
    let shared = fm.usage_window();
    let window = dashboard.effective_usage_window(shared.as_ref()).unwrap();
    assert_eq!(window.from.as_ref().unwrap().date, Date::new(2026, 6, 1));
    assert_eq!(window.to.as_ref().unwrap().date, Date::new(2026, 6, 30));
}

#[test]
fn footnote_labels_resolve_to_source_ids() {
    let tmp = finance_bundle();
    let bundle = Bundle::load(tmp.path()).unwrap();
    let revenue = bundle.get(&id("computations/revenue")).unwrap();

    let attributions = revenue.document.attributions();
    assert_eq!(attributions.len(), 2);
    assert!(attributions.iter().all(okf::Attribution::is_resolved));
    assert_eq!(attributions[0].label, "rev-policy");
    assert_eq!(attributions[0].references, 1);
    assert_eq!(attributions[0].definitions, 1);
    assert_eq!(
        attributions[1].source.as_ref().unwrap().title.as_deref(),
        Some("Executive revenue dashboard")
    );

    // The `[^rev-policy]` marker is not mistaken for a markdown link.
    assert!(revenue.document.links().is_empty());
}

#[test]
fn attested_computation_contracts_are_read_in_full() {
    let tmp = finance_bundle();
    let bundle = Bundle::load(tmp.path()).unwrap();

    let ids: Vec<String> = bundle
        .attested_computations()
        .map(|c| c.id.to_string())
        .collect();
    assert_eq!(ids, vec!["computations/profit", "computations/revenue"]);

    let revenue = bundle
        .get(&id("computations/revenue"))
        .unwrap()
        .attested_computation()
        .unwrap();
    assert_eq!(revenue.runtime.as_deref(), Some("bigquery"));
    assert_eq!(revenue.required_parameters().count(), 1);
    assert_eq!(
        revenue.executor.as_ref().unwrap().receipt,
        vec!["job_id", "executed_sql", "result"]
    );
    assert!(matches!(revenue.computation, ComputationSource::Inline(_)));
    assert_eq!(
        revenue.computation.code().unwrap(),
        "SELECT SUM(amount) AS revenue\nFROM finance.recognized_revenue\nWHERE fiscal_year = @year"
    );

    // A dbt runtime changes what parameters mean, not how they are declared.
    let profit = bundle
        .get(&id("computations/profit"))
        .unwrap()
        .attested_computation()
        .unwrap();
    assert_eq!(profit.runtime.as_deref(), Some("dbt"));
    assert_eq!(profit.parameters.len(), 2);
    assert_eq!(profit.parameters[1].name.as_deref(), Some("segment"));
    assert!(profit
        .executor
        .as_ref()
        .unwrap()
        .receipt
        .contains(&"compiled_sql".to_string()));
    assert!(profit
        .computation
        .code()
        .unwrap()
        .contains("{{ ref('fct_income_statement') }}"));
}

#[test]
fn contract_paths_resolve_from_the_bundle_root() {
    let tmp = finance_bundle();
    let bundle = Bundle::load(tmp.path()).unwrap();
    let revenue_id = id("computations/revenue");

    // The spec writes `references/...` from the bundle root even though the
    // concept lives in `computations/` (§6.2, §6.3).
    let executor = bundle
        .resolve_path_field(&revenue_id, "references/skills/run-on-bq.md")
        .unwrap();
    assert!(executor.ends_with("references/skills/run-on-bq.md"));

    let attester = bundle
        .resolve_path_field(&revenue_id, "references/attesters/sql-equality.py")
        .unwrap();
    assert!(attester.exists(), "non-markdown attester code resolves too");

    assert!(bundle
        .resolve_path_field(&revenue_id, "references/nope.py")
        .is_none());
    assert!(bundle
        .resolve_path_field(&revenue_id, "https://example.com")
        .is_none());
}

#[test]
fn tag_view_is_synthesized_from_frontmatter() {
    let tmp = finance_bundle();
    let bundle = Bundle::load(tmp.path()).unwrap();

    let tags = bundle.tags();
    assert_eq!(
        tags.keys().collect::<Vec<_>>(),
        vec!["finance", "income-statement", "profit", "revenue"]
    );
    assert_eq!(tags["finance"].len(), 3);
    assert_eq!(tags["revenue"], vec![id("computations/revenue")]);
}

#[test]
fn source_resources_that_name_concepts_become_derivation_edges() {
    let tmp = TempDir::new();
    tmp.write(
        "tables/orders.md",
        "---\ntype: BigQuery Table\ntitle: Orders\n---\n\nBase table.\n",
    );
    tmp.write(
        "metrics/aov.md",
        "---\ntype: Metric\ntitle: Average order value\n\
         sources:\n\
         \x20 - { id: orders, resource: /tables/orders.md, title: Orders }\n\
         \x20 - { id: external, resource: https://wiki.acme/aov, title: AOV definition }\n\
         \x20 - { id: scope, resource: all queries in BigQuery project X }\n---\n\nProse.\n",
    );
    let bundle = Bundle::load(tmp.path()).unwrap();

    let aov = id("metrics/aov");
    let resolved = bundle.sources_of(&aov);
    assert_eq!(resolved.len(), 3);
    assert_eq!(resolved[0].concept, Some(id("tables/orders")));
    assert_eq!(resolved[1].concept, None, "a URL is not a bundle concept");
    assert_eq!(
        resolved[2].concept, None,
        "a scope descriptor is not a path"
    );
    assert_eq!(resolved[2].source.resource_kind(), ResourceKind::Scope);

    assert_eq!(bundle.derived_from(&aov), vec![&id("tables/orders")]);
    assert_eq!(bundle.derives(&id("tables/orders")), &[aov]);
}

#[test]
fn staleness_reporting_is_opt_in_and_deterministic() {
    let tmp = finance_bundle();
    let bundle = Bundle::load(tmp.path()).unwrap();

    // No date supplied: nothing is reported stale, whatever the clock says.
    let plain = validate_bundle(&bundle);
    assert!(!plain
        .of(Severity::Info)
        .any(|d| d.message.contains("stale since")));

    let dated = validate_bundle_at(&bundle, Date::new(2026, 7, 1));
    let stale: Vec<&str> = dated
        .of(Severity::Info)
        .filter(|d| d.message.contains("stale since"))
        .map(|d| d.message.as_str())
        .collect();
    assert_eq!(stale.len(), 1);
    assert!(stale[0].contains("2026-06-15"), "{}", stale[0]);
    assert!(
        dated.is_conformant(),
        "staleness is informational, not a violation"
    );
}

#[test]
fn v0_1_documents_still_load_under_the_documented_fallbacks() {
    // The Appendix A "v0.1 form": one concept, a `timestamp`, and a body
    // citations list (§13.1).
    let doc = Document::parse(
        "---\n\
         type: Metric\n\
         title: Income statement (fiscal year)\n\
         description: Headline income-statement figures for a fiscal year.\n\
         tags: [finance, income-statement]\n\
         timestamp: '2026-05-28T22:53:05+00:00'\n\
         ---\n\n\
         # Definition\n\
         The income statement reports revenue and gross profit.\n\n\
         # Citations\n\
         - https://wiki.acme/finance/fpa-handbook\n",
    )
    .unwrap();

    assert!(doc.validate().is_ok());
    // `generated` is absent, so `timestamp` stands in for `generated.at`.
    assert!(doc.frontmatter.generated().is_none());
    let changed = doc.frontmatter.content_changed_at().unwrap();
    assert_eq!(
        changed.datetime.unwrap().date,
        Date::new(2026, 5, 28).unwrap()
    );
    // Absent trust frontmatter has meaning, and is never a rejection (§11).
    assert_eq!(doc.frontmatter.trust_tier(), TrustTier::Unverified);
    assert_eq!(doc.frontmatter.status(), Status::Stable);
    assert_eq!(doc.frontmatter.legacy_keys(), vec!["timestamp"]);
}

#[test]
fn v0_1_bundles_are_conformant_but_flagged_for_migration() {
    let tmp = TempDir::new();
    tmp.write(
        "metric.md",
        "---\ntype: Metric\ntitle: Revenue\ndescription: Recognized revenue.\n\
         timestamp: 2026-05-28T22:53:05+00:00\n---\n\n\
         Prose.\n\n# Citations\n[1] [Policy](https://wiki.acme/policy)\n",
    );
    let bundle = Bundle::load(tmp.path()).unwrap();
    let report = validate_bundle(&bundle);

    assert!(
        report.is_conformant(),
        "a v0.1 bundle is still a conformant v0.2 bundle"
    );
    let warnings: Vec<&str> = report
        .of(Severity::Warning)
        .map(|d| d.message.as_str())
        .collect();
    assert!(
        warnings
            .iter()
            .any(|w| w.contains("`timestamp` is superseded")),
        "{warnings:?}"
    );
    assert!(
        warnings.iter().any(|w| w.contains("`# Citations`")),
        "{warnings:?}"
    );
}

#[test]
fn malformed_families_warn_without_breaking_conformance() {
    let tmp = TempDir::new();
    tmp.write(
        "bad.md",
        "---\n\
         type: Attested Computation\n\
         title: Broken\n\
         description: Every family mis-specified.\n\
         tags: finance, revenue\n\
         status: experimental\n\
         stale_after: soon\n\
         generated: { at: 2026-06-20T22:53:05Z }\n\
         verified: [{ by: human:ahormati, at: yesterday }]\n\
         sources:\n\
         \x20 - { id: dup, resource: https://a }\n\
         \x20 - { id: dup, title: no resource }\n\
         ---\n\n\
         A claim.[^unknown]\n\n\
         [^unknown]: Not a source id\n",
    );
    let bundle = Bundle::load(tmp.path()).unwrap();
    let report = validate_bundle(&bundle);

    assert!(
        report.is_conformant(),
        "`type` is present, so §11 is satisfied"
    );
    let warnings: Vec<&str> = report
        .of(Severity::Warning)
        .map(|d| d.message.as_str())
        .collect();
    let has = |needle: &str| {
        assert!(
            warnings.iter().any(|w| w.contains(needle)),
            "missing {needle:?} in {warnings:#?}"
        );
    };
    has("`tags` should be a list of short strings");
    has("unknown `status` value");
    has("`stale_after` is not an absolute");
    has("`generated.by` is required");
    has("`verified[0].at` is not an ISO-8601 datetime");
    has("duplicate `sources[].id`");
    has("`sources[1].resource` is required");
    has("matches no `sources[].id`");
    has("`runtime` is required");
    has("no computation");
    has("missing `executor`");
    has("missing `attester`");
}

#[test]
fn declared_version_is_reported_never_enforced() {
    for (declared, needle) in [
        ("0.1", "targets OKF v0.1"),
        ("0.9", "unrecognized `okf_version: 0.9`"),
    ] {
        let tmp = TempDir::new();
        tmp.write("a.md", "---\ntype: Note\n---\nbody\n");
        tmp.write(
            "index.md",
            &format!("---\nokf_version: \"{declared}\"\n---\n\n# Listing\n"),
        );
        let bundle = Bundle::load(tmp.path()).unwrap();
        let report = validate_bundle(&bundle);
        assert!(
            report.is_conformant(),
            "a version mismatch is never a violation (§12)"
        );
        assert!(
            report
                .of(Severity::Info)
                .any(|d| d.message.contains(needle)),
            "{declared}: {:#?}",
            report.diagnostics
        );
    }
}

#[test]
fn computation_keys_on_another_type_are_noted() {
    let tmp = TempDir::new();
    tmp.write(
        "metric.md",
        "---\ntype: Metric\ntitle: Revenue\nruntime: bigquery\n---\n\nProse.\n",
    );
    let bundle = Bundle::load(tmp.path()).unwrap();
    let report = validate_bundle(&bundle);
    assert!(report.of(Severity::Info).any(|d| d
        .message
        .contains("a sanctioned computation is its own concept")));
}

#[test]
fn every_frontmatter_family_round_trips() {
    let doc = Document::parse(REVENUE).unwrap();
    let reparsed = Document::parse(&doc.serialize()).unwrap();
    assert_eq!(reparsed.frontmatter, doc.frontmatter);

    // Flow mappings re-emit as block mappings, which is the same value.
    let contract = reparsed.attested_computation().unwrap();
    assert_eq!(contract.runtime.as_deref(), Some("bigquery"));
    assert_eq!(reparsed.frontmatter.trust_tier(), TrustTier::HumanReviewed);
    assert_eq!(reparsed.frontmatter.sources().len(), 2);
    assert!(reparsed.frontmatter.extension_keys().is_empty());
}

#[test]
fn regenerating_indexes_keeps_the_declared_version() {
    let tmp = finance_bundle();
    okf::index::regenerate_indexes(tmp.path()).unwrap();

    let root = tmp.read("index.md");
    assert!(
        root.starts_with("---\nokf_version: \"0.2\"\n---\n"),
        "{root}"
    );
    assert!(root.contains("# Subdirectories"), "{root}");

    let bundle = Bundle::load(tmp.path()).unwrap();
    assert_eq!(bundle.okf_version().as_deref(), Some("0.2"));

    // Attested Computations get their own index section, which is how §10.5's
    // discovery-by-type works from an index.
    let computations = tmp.read("computations/index.md");
    assert!(
        computations.starts_with("# Attested Computation"),
        "{computations}"
    );
}
