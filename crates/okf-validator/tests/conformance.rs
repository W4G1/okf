//! End-to-end conformance tests for [`validate_bundle`] and conformance clauses,
//! driven by the specification's Appendix A worked example: an income statement
//! whose two figures are attested computations, with every frontmatter family
//! populated and the two computations deliberately left in different trust and
//! freshness states.
//!
//! The documents below are transcribed from the OKF v0.3 spec, so these tests
//! double as a fidelity check: if the crate cannot read the spec's own
//! examples, it does not implement the spec. The reference implementation has
//! no counterpart to this file, since it validates a document at a time rather
//! than a bundle.

mod common;

use common::TempDir;
use okf_core::{
    ActorKind, Bundle, ComputationSource, ConceptId, Date, Document, Freshness, LinkTarget,
    PathResolution, ResourceKind, Status, TrustTier,
};
use okf_validator::{Severity, validate_bundle, validate_bundle_at};

const INDEX: &str = r#"---
okf_version: "0.3"
title: Acme finance
description: Income-statement metrics and their sanctioned computations. Use it for revenue and profit questions.
license: CC-BY-4.0
---

# Metrics

* [Income statement (fiscal year)](metrics/income-statement.md) - Headline income-statement figures for a fiscal year.

# Computations

* [Revenue for fiscal year](computations/revenue.md) - Recognized revenue for a fiscal year, per Finance's definition.
* [Gross profit for fiscal year](computations/profit.md) - Gross profit by segment for a fiscal year, per the cost-allocation standard.
"#;

const INCOME_STATEMENT: &str = r#"---
type: Metric
title: Income statement (fiscal year)
description: Headline income-statement figures for a fiscal year.
tags: [finance, income-statement]
status: stable
generated: { by: reference_agent/gemini-2.5-pro, at: "2026-06-20T22:53:05Z" }
verified: { by: "human:ahormati", at: "2026-06-25T09:00:00Z" }
stale_after: "2026-12-31T00:00:00Z"
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
"#;

const REVENUE: &str = r#"---
type: Attested Computation
title: Revenue for fiscal year
description: Recognized revenue for a fiscal year, per Finance's definition.
tags: [finance, revenue]
status: stable
runtime: bigquery
parameters:
  - { name: year, type: integer, required: true }
executor:
  resource: /references/skills/run-on-bq.md
  receipt: [job_id, executed_sql, result]
attester:
  resource: /references/attesters/sql-equality.py
generated: { by: reference_agent/gemini-2.5-pro, at: "2026-06-20T14:00:00Z" }
verified: { by: "human:ahormati", at: "2026-06-25T09:00:00Z" }
stale_after: "2026-12-31T00:00:00Z"
sources:
  - id: rev-policy
    resource: https://wiki.acme/finance/revenue-recognition
    title: Revenue recognition policy
    author: "team:finance-fpa"
    last_modified: "2026-04-02T00:00:00Z"
  - id: exec-rev-dash
    resource: https://bi.acme.example/dashboards/exec-revenue
    title: Executive revenue dashboard
    author: "team:finance-fpa"
    usage_count: 5000
    last_modified: "2026-06-18T00:00:00Z"
usage_window: { from: "2026-06-01T00:00:00Z", to: "2026-06-30T00:00:00Z" }
---

# Computation

```sql
SELECT SUM(amount) AS revenue
FROM finance.recognized_revenue
WHERE fiscal_year = @year
```

Recognized revenue per the recognition policy,[^rev-policy] corroborated by
the executive revenue dashboard.[^exec-rev-dash]

[^rev-policy]: Revenue recognition policy
[^exec-rev-dash]: Executive revenue dashboard
"#;

const PROFIT: &str = r#"---
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
  resource: /references/skills/run-dbt.md
  receipt: [run_id, compiled_sql, result]
attester:
  resource: /references/attesters/dbt-binding.py
generated: { by: reference_agent/gemini-2.5-pro, at: "2026-06-10T14:00:00Z" }
verified: { by: "process:finance-nightly", at: "2026-06-12T08:00:00Z" }
stale_after: "2026-06-15T00:00:00Z"
sources:
  - id: cost-alloc
    resource: https://wiki.acme/finance/cost-allocation
    title: Cost allocation standard
---

# Computation

```sql
SELECT gross_profit
FROM {{ ref('fct_income_statement') }}
WHERE fiscal_year = {{ var('year') }}
  AND segment = {{ var('segment') }}
```

Gross profit by segment per the cost-allocation standard.[^cost-alloc]

[^cost-alloc]: Cost allocation standard
"#;

/// The Appendix A bundle, `bundles/finance/`.
///
/// The run instructions are plain markdown without frontmatter and the
/// attesters are Python, so none of the four files under `references/` is a
/// concept: a path field may point at any file in the bundle.
fn finance_bundle() -> TempDir {
    let tmp = TempDir::new();
    tmp.write("index.md", INDEX);
    tmp.write("metrics/income-statement.md", INCOME_STATEMENT);
    tmp.write("computations/revenue.md", REVENUE);
    tmp.write("computations/profit.md", PROFIT);
    tmp.write(
        "references/skills/run-on-bq.md",
        "# Run on BigQuery\n\nRun instructions.\n",
    );
    tmp.write(
        "references/skills/run-dbt.md",
        "# Run dbt\n\nRun instructions.\n",
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

    // The two run instructions have no frontmatter and the two attesters are
    // not markdown, so none of them is a concept.
    assert_eq!(bundle.len(), 3);
    assert_eq!(bundle.non_concepts().len(), 2);
    assert!(bundle.parse_errors().is_empty());
    assert_eq!(bundle.okf_version(), Some("0.3"));
    assert!(!bundle.legacy_fallbacks_apply());

    let metadata = bundle.metadata();
    assert_eq!(metadata.title.as_deref(), Some("Acme finance"));
    assert_eq!(metadata.license.as_deref(), Some("CC-BY-4.0"));
    assert!(
        metadata
            .description
            .as_deref()
            .unwrap()
            .starts_with("Income-statement metrics")
    );

    let report = validate_bundle(&bundle);
    assert!(report.is_conformant(), "{:#?}", report.diagnostics);
    assert_eq!(report.error_count(), 0);
    // The spec's own example is clean but for the two files that are not
    // concepts, which a validator should point out.
    let warnings: Vec<&str> = report
        .of(Severity::Warning)
        .map(|d| d.message.as_str())
        .collect();
    assert_eq!(warnings.len(), 2, "{warnings:#?}");
    assert!(warnings.iter().all(|w| w.contains("not a concept")));
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

    // Each computation knows which concepts use it (discovery path).
    assert_eq!(bundle.backlinks(&id("computations/revenue")), &[statement]);
}

#[test]
fn appendix_a_results_table() {
    // "A consumer that reads this bundle on or after 2026-06-15 and before
    // 2026-12-31 gets these results":
    //
    // | Concept                    | Trust tier        | Fresh | Attestable |
    // | metrics/income-statement   | human-reviewed    | yes   | no         |
    // | computations/revenue       | human-reviewed    | yes   | yes        |
    // | computations/profit        | machine-confirmed | no    | yes        |
    let tmp = finance_bundle();
    let bundle = Bundle::load(tmp.path()).unwrap();
    let now = Date::new(2026, 7, 1).unwrap().to_utc_datetime();

    for (concept, tier, freshness, attestable) in [
        (
            "metrics/income-statement",
            TrustTier::HumanReviewed,
            Freshness::Fresh,
            false,
        ),
        (
            "computations/revenue",
            TrustTier::HumanReviewed,
            Freshness::Fresh,
            true,
        ),
        (
            "computations/profit",
            TrustTier::MachineConfirmed,
            Freshness::Stale,
            true,
        ),
    ] {
        let found = bundle.get(&id(concept)).unwrap();
        assert_eq!(found.trust_tier(), tier, "{concept}");
        assert_eq!(found.freshness_at(now), freshness, "{concept}");
        assert_eq!(bundle.is_attestable(&id(concept)), attestable, "{concept}");
        assert!(found.is_current(), "{concept}");
    }
}

#[test]
fn trust_tiers_and_freshness_differ_per_computation() {
    let tmp = finance_bundle();
    let bundle = Bundle::load(tmp.path()).unwrap();

    let revenue = bundle.get(&id("computations/revenue")).unwrap();
    let profit = bundle.get(&id("computations/profit")).unwrap();

    // A human signed off revenue; only a process signed off profit.
    assert_eq!(revenue.trust_tier(), TrustTier::HumanReviewed);
    assert_eq!(profit.trust_tier(), TrustTier::MachineConfirmed);
    assert!(revenue.trust_tier() > profit.trust_tier());
    assert_eq!(revenue.status(), Status::Stable);

    // "Revenue can be fresh while profit is past its stale_after".
    let today = Date::new(2026, 7, 1).unwrap();
    assert!(!revenue.is_stale_on(today));
    assert!(profit.is_stale_on(today));
    let stale = bundle.stale_on(today);
    assert_eq!(stale.len(), 1);
    assert_eq!(stale[0].id, id("computations/profit"));

    // A bare `verified` mapping is one event, not a parse failure.
    let verified = revenue.document.frontmatter.verified();
    assert_eq!(verified.len(), 1);
    assert_eq!(verified[0].by.as_ref().unwrap().kind(), ActorKind::Human);

    let generated = revenue.document.frontmatter.generated().unwrap();
    assert_eq!(
        generated.by.as_ref().unwrap().producer(),
        Some("reference_agent")
    );
    // The verification is after the last change, so it is current.
    let generated_at = generated.at.unwrap().instant().unwrap();
    let verified_at = verified[0].at.as_ref().unwrap().instant().unwrap();
    assert!(verified_at > generated_at);
    assert!(!revenue.document.frontmatter.verified_before_change());
}

#[test]
fn a_rewrite_after_review_costs_the_human_reviewed_tier() {
    // The one change to `generated.at` below is what an agent does when it
    // regenerates a concept. The review that was current no longer is.
    let tmp = finance_bundle();
    tmp.write(
        "computations/revenue.md",
        &REVENUE.replace("2026-06-20T14:00:00Z", "2026-06-28T14:00:00Z"),
    );
    let bundle = Bundle::load(tmp.path()).unwrap();
    let revenue = bundle.get(&id("computations/revenue")).unwrap();

    assert_eq!(revenue.trust_tier(), TrustTier::Unverified);
    assert!(revenue.document.frontmatter.verified_before_change());
    // The event stays in the file as history.
    assert_eq!(revenue.document.frontmatter.verified().len(), 1);

    let report = validate_bundle(&bundle);
    assert!(report.is_conformant());
    let notes: Vec<_> = report
        .of(Severity::Info)
        .filter(|d| d.message.contains("no verification is current"))
        .collect();
    assert_eq!(notes.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(notes[0].concept, Some(id("computations/revenue")));
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
    assert_eq!(policy.author.as_ref().unwrap().kind(), ActorKind::Team);
    assert_eq!(
        policy
            .last_modified
            .as_ref()
            .unwrap()
            .instant()
            .unwrap()
            .date,
        Date::new(2026, 4, 2).unwrap()
    );
    assert_eq!(policy.usage_count, None);

    // The shared window frames the dashboard's usage_count.
    let dashboard = &sources[1];
    assert_eq!(dashboard.resource_kind(), ResourceKind::Url);
    assert_eq!(dashboard.usage_count, Some(5000));
    let shared = fm.usage_window();
    let window = dashboard.effective_usage_window(shared.as_ref()).unwrap();
    assert_eq!(
        window.from.as_ref().unwrap().instant().unwrap().date,
        Date::new(2026, 6, 1).unwrap()
    );
    assert_eq!(
        window.to.as_ref().unwrap().instant().unwrap().date,
        Date::new(2026, 6, 30).unwrap()
    );
}

#[test]
fn footnote_labels_resolve_to_source_ids() {
    let tmp = finance_bundle();
    let bundle = Bundle::load(tmp.path()).unwrap();
    let revenue = bundle.get(&id("computations/revenue")).unwrap();

    let attributions = revenue.document.attributions();
    assert_eq!(attributions.len(), 2);
    assert!(attributions.iter().all(okf_core::Attribution::is_resolved));
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
    assert!(matches!(
        &revenue.computation,
        ComputationSource::Inline(inline) if inline.fenced && inline.language.as_deref() == Some("sql")
    ));
    assert!(revenue.is_attestable());
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
    assert!(
        profit
            .executor
            .as_ref()
            .unwrap()
            .receipt
            .contains(&"compiled_sql".to_string())
    );
    assert!(
        profit
            .computation
            .code()
            .unwrap()
            .contains("{{ ref('fct_income_statement') }}")
    );
}

#[test]
fn contract_paths_resolve_like_links() {
    let tmp = finance_bundle();
    let bundle = Bundle::load(tmp.path()).unwrap();
    let revenue_id = id("computations/revenue");

    // A path that starts with `/` resolves from the bundle root, wherever the
    // concept is.
    let executor = bundle
        .resolve_path_field(&revenue_id, "/references/skills/run-on-bq.md")
        .unwrap();
    assert!(executor.ends_with("references/skills/run-on-bq.md"));

    let attester = bundle
        .resolve_path_field(&revenue_id, "/references/attesters/sql-equality.py")
        .unwrap();
    assert!(attester.exists(), "non-markdown attester code resolves too");

    // Any other path resolves from the directory of the concept's file.
    assert!(matches!(
        bundle.resolve_path(&revenue_id, "../references/attesters/sql-equality.py"),
        PathResolution::Found(_)
    ));
    assert!(matches!(
        bundle.resolve_path(&revenue_id, "profit.md"),
        PathResolution::Found(_)
    ));
    // A directory can be behind a `resource`: a skill is often a folder.
    assert!(matches!(
        bundle.resolve_path(&revenue_id, "/references/skills"),
        PathResolution::Found(_)
    ));

    // This bundle declares 0.3, so a bare path is not read from the root.
    assert_eq!(
        bundle.resolve_path(&revenue_id, "references/skills/run-on-bq.md"),
        PathResolution::Missing
    );
    // But it is recognizable as one that was meant that way.
    assert_eq!(
        bundle
            .rooted_path_suggestion(&revenue_id, "references/skills/run-on-bq.md")
            .as_deref(),
        Some("/references/skills/run-on-bq.md")
    );

    assert_eq!(
        bundle.resolve_path(&revenue_id, "/references/nope.py"),
        PathResolution::Missing
    );
    assert_eq!(
        bundle.resolve_path(&revenue_id, "https://example.com"),
        PathResolution::External
    );
}

#[test]
fn v0_2_bundles_resolve_bare_path_fields_from_the_root_as_a_fallback() {
    // The v0.2 examples wrote path fields from the bundle root without a
    // leading `/`. A bundle that declares 0.2, or nothing, still reads.
    let legacy_revenue = REVENUE
        .replace("resource: /references/", "resource: references/")
        .replace(
            "https://bi.acme.example/dashboards/exec-revenue",
            "metrics/income-statement.md",
        );
    for declaration in ["---\nokf_version: \"0.2\"\n---\n\n", ""] {
        let tmp = finance_bundle();
        tmp.write("index.md", &format!("{declaration}# Finance\n"));
        tmp.write("computations/revenue.md", &legacy_revenue);
        let bundle = Bundle::load(tmp.path()).unwrap();
        let revenue_id = id("computations/revenue");
        assert!(bundle.legacy_fallbacks_apply());

        match bundle.resolve_path(&revenue_id, "references/skills/run-on-bq.md") {
            PathResolution::Fallback(path) => {
                assert!(path.ends_with("references/skills/run-on-bq.md"));
            }
            other => panic!("expected the root fallback, found {other:?}"),
        }
        assert!(bundle.is_attestable(&revenue_id));
        // A source written from the root is still a derivation edge.
        assert_eq!(
            bundle.derived_from(&revenue_id),
            vec![&id("metrics/income-statement")]
        );

        let report = validate_bundle(&bundle);
        assert!(report.is_conformant(), "{:#?}", report.diagnostics);
        let fallback: Vec<_> = report
            .of(Severity::Warning)
            .filter(|d| d.message.contains("resolves only from the bundle root"))
            .collect();
        assert_eq!(fallback.len(), 3, "{:#?}", report.diagnostics);
        assert!(fallback.iter().all(|d| d.fixable));
        assert!(fallback.iter().any(|d| {
            d.message
                .contains("write it as `/references/skills/run-on-bq.md`")
        }));
    }

    // Declare 0.3 and the same values no longer resolve. The validator still
    // recognizes what was meant, and can fix it.
    let tmp = finance_bundle();
    tmp.write("computations/revenue.md", &legacy_revenue);
    let bundle = Bundle::load(tmp.path()).unwrap();
    let revenue_id = id("computations/revenue");
    assert!(!bundle.legacy_fallbacks_apply());
    assert_eq!(
        bundle.resolve_path(&revenue_id, "references/skills/run-on-bq.md"),
        PathResolution::Missing
    );
    assert!(bundle.derived_from(&revenue_id).is_empty());
    let report = validate_bundle(&bundle);
    let unresolved: Vec<_> = report
        .of(Severity::Warning)
        .filter(|d| d.message.contains("would resolve from the bundle root"))
        .collect();
    assert_eq!(unresolved.len(), 3, "{:#?}", report.diagnostics);
    assert!(unresolved.iter().all(|d| d.fixable));
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
    assert!(
        !plain
            .of(Severity::Info)
            .any(|d| d.message.contains("stale since"))
    );

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
    // citations list.
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
        changed.instant().unwrap().date,
        Date::new(2026, 5, 28).unwrap()
    );
    // Absent trust frontmatter has meaning, and is never a rejection.
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
        "a v0.1 bundle is still a conformant bundle"
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
         superseded_by: /nowhere.md\n\
         stale_after: soon\n\
         generated: { at: 2026-06-20T22:53:05Z }\n\
         verified: [{ by: human:walter, at: yesterday }, { by: Finance Team }]\n\
         sources:\n\
         \x20 - { id: dup, resource: https://a, author: Finance Team }\n\
         \x20 - { id: dup, title: no resource }\n\
         \x20 - { id: dup, resource: https://b, usage_count: 5 }\n\
         parameters:\n\
         \x20 - { name: year }\n\
         executor: { receipt: job_id }\n\
         attester: {}\n\
         ---\n\n\
         A claim.[^unknown]\n\n\
         [^unknown]: Not a source id\n",
    );
    let bundle = Bundle::load(tmp.path()).unwrap();
    let report = validate_bundle(&bundle);

    assert!(
        report.is_conformant(),
        "`type` is present and the frontmatter parses, so conformance is satisfied: {:#?}",
        report.diagnostics
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
    has("`tags` should be a YAML list");
    has("consumers read it as the single tag \"finance, revenue\"");
    has("unknown `status` value");
    has("`superseded_by` has a meaning only with `status: deprecated`");
    has("`superseded_by` does not resolve to a file in the bundle");
    has("`stale_after` does not parse");
    has("`generated.by` is required");
    has("`verified[0].at` is not an RFC 3339 date-time");
    has("`verified[1]` is missing `at`");
    has("`verified[1].by` is not an actor");
    has("`sources[0].author` is not an actor");
    has("duplicate `sources[].id`");
    has("`sources[1].resource` is required");
    has("`sources[2].usage_count` needs a `usage_window`");
    has("matches no `sources[].id`");
    has("`runtime` is required");
    has("no computation");
    has("`parameters[0]` is missing `type`");
    has("`executor.resource` is missing");
    has("`attester.resource` is missing");

    // The reader applies the same rules the validator describes.
    let fm = &bundle.concepts()[0].document.frontmatter;
    assert_eq!(fm.tags(), vec!["finance, revenue"]);
    assert!(!fm.is_current());
    assert!(fm.is_stale_on(Date::new(2000, 1, 1).unwrap()));
    assert_eq!(fm.sources().len(), 2);
    assert!(fm.verified().is_empty() || fm.trust_tier() == TrustTier::Unverified);
    assert!(fm.parameters().is_empty());
    assert!(!bundle.is_attestable(&bundle.concepts()[0].id));
}

#[test]
fn malformed_sources_members_are_diagnosed_without_being_conformance_errors() {
    let tmp = TempDir::new();
    tmp.write(
        "bad.md",
        "---\n\
         type: Note\n\
         sources:\n\
         \x20 - { id: good, resource: https://example.com }\n\
         \x20 - not-a-source-entry\n\
         \x20 - 42\n\
         ---\n\nBody.\n",
    );
    let bundle = Bundle::load(tmp.path()).unwrap();
    let report = validate_bundle(&bundle);

    assert!(report.is_conformant(), "{:#?}", report.diagnostics);
    assert!(
        report
            .of(Severity::Warning)
            .any(|d| { d.message.contains("`sources[1]`") && d.message.contains("mapping entry") })
    );
    assert!(
        report
            .of(Severity::Warning)
            .any(|d| { d.message.contains("`sources[2]`") && d.message.contains("mapping entry") })
    );
}

#[test]
fn a_file_without_a_usable_type_is_not_a_concept_and_not_an_error() {
    let tmp = TempDir::new();
    tmp.write("list.md", "---\ntype: [Metric, Note]\n---\nBody.\n");
    tmp.write("none.md", "---\ntitle: No type at all\n---\nBody.\n");
    tmp.write("README.md", "# Read me\n");
    // A number is not a string, but it is a non-empty type: the concept is
    // kept, and the producer is told to quote it.
    tmp.write("number.md", "---\ntype: 42\n---\nBody.\n");
    let bundle = Bundle::load(tmp.path()).unwrap();

    assert!(bundle.concepts_of_type("Metric").next().is_none());
    assert_eq!(bundle.len(), 1);
    assert_eq!(bundle.concepts_of_type("42").count(), 1);
    assert_eq!(bundle.non_concepts().len(), 3);

    let report = validate_bundle(&bundle);
    assert!(report.is_conformant(), "{:#?}", report.diagnostics);
    let not_concepts: Vec<&str> = report
        .of(Severity::Warning)
        .filter(|d| d.message.contains("not a concept"))
        .map(|d| d.message.as_str())
        .collect();
    assert_eq!(not_concepts.len(), 3, "{not_concepts:#?}");
    assert_eq!(
        not_concepts
            .iter()
            .filter(|m| m.contains("no frontmatter"))
            .count(),
        1
    );
    assert!(
        report
            .of(Severity::Warning)
            .any(|d| d.message.contains("`type` is an integer, not a string"))
    );
}

#[test]
fn unparseable_frontmatter_and_repeated_keys_are_conformance_errors() {
    // Conformance asks one thing of a frontmatter block: that it parses as a
    // YAML mapping. That holds for every markdown file, concept or not.
    let tmp = TempDir::new();
    tmp.write("ok.md", "---\ntype: Note\n---\nBody.\n");
    tmp.write("unclosed.md", "---\ntype: Note\nBody.\n");
    tmp.write("list.md", "---\n- a\n- b\n---\nBody.\n");
    tmp.write("broken.md", "---\ntags: [a, b\n---\nBody.\n");
    let report = validate_bundle(&Bundle::load(tmp.path()).unwrap());
    assert!(!report.is_conformant());
    assert_eq!(
        report
            .of(Severity::Error)
            .filter(|d| d.message.contains("does not parse as a YAML mapping"))
            .count(),
        3,
        "{:#?}",
        report.diagnostics
    );

    // Keys must be unique, at any depth, in a concept or any other file.
    let tmp = TempDir::new();
    tmp.write(
        "dup.md",
        "---\ntype: Note\ntitle: One\ntitle: Two\ngenerated: { by: a/b, by: c/d }\n---\nBody.\n",
    );
    tmp.write("untyped.md", "---\ntitle: One\ntitle: Two\n---\nBody.\n");
    let bundle = Bundle::load(tmp.path()).unwrap();
    // The loader is lenient and reads the first occurrence.
    assert_eq!(
        bundle.concepts()[0].document.frontmatter.title().as_deref(),
        Some("One")
    );
    let report = validate_bundle(&bundle);
    assert!(!report.is_conformant());
    let repeated: Vec<&str> = report
        .of(Severity::Error)
        .map(|d| d.message.as_str())
        .collect();
    assert_eq!(repeated.len(), 3, "{repeated:#?}");
    assert!(repeated.iter().any(|m| m.contains("`generated.by`")));
    assert_eq!(repeated.iter().filter(|m| m.contains("`title`")).count(), 2);
}

#[test]
fn malformed_verification_events_do_not_elevate_trust_or_hide_diagnostics() {
    let tmp = TempDir::new();
    tmp.write(
        "bad.md",
        "---\n\
         type: Note\n\
         verified:\n\
         \x20 - { by: human:invalid, at: 2026-06-26T09:00:00 }\n\
         \x20 - { by: human:also-invalid, at: yesterday }\n\
         \x20 - not-an-event\n\
         \x20 - { by: human:no-time }\n\
         ---\n\nBody.\n",
    );
    let bundle = Bundle::load(tmp.path()).unwrap();
    let concept = bundle.concepts().first().unwrap();
    assert_eq!(concept.trust_tier(), TrustTier::Unverified);

    let report = validate_bundle(&bundle);
    // A date-time without an offset is close enough to repair.
    assert!(report.of(Severity::Warning).any(|d| {
        d.message
            .contains("`verified[0].at` is not an RFC 3339 date-time")
            && d.fixable
    }));
    assert!(report.of(Severity::Warning).any(|d| {
        d.message
            .contains("`verified[1].at` is not an RFC 3339 date-time")
            && !d.fixable
    }));
    assert!(
        report
            .of(Severity::Warning)
            .any(|d| d.message.contains("`verified[2]` should be a mapping"))
    );
    assert!(
        report
            .of(Severity::Warning)
            .any(|d| d.message.contains("`verified[3]` is missing `at`"))
    );
}

#[test]
fn a_date_only_timestamp_is_valid_and_means_the_start_of_that_day_in_utc() {
    let tmp = TempDir::new();
    tmp.write(
        "dated.md",
        "---\n\
         type: Note\n\
         generated: { by: tool/v1, at: 2026-06-26 }\n\
         verified: { by: human:reviewer, at: \"2026-06-26\" }\n\
         stale_after: 2026-12-31\n\
         sources:\n\
         \x20 - { resource: https://example.com, last_modified: 2026-05-15, usage_count: 3 }\n\
         usage_window: { from: 2026-06-01, to: 2026-06-30 }\n\
         ---\n\nBody.\n",
    );
    let bundle = Bundle::load(tmp.path()).unwrap();
    let report = validate_bundle(&bundle);

    assert!(report.is_conformant(), "{:#?}", report.diagnostics);
    // The review and the change name the same instant, so the review is
    // current.
    assert_eq!(bundle.concepts()[0].trust_tier(), TrustTier::HumanReviewed);
    assert!(!bundle.concepts()[0].is_stale_on(Date::new(2026, 12, 30).unwrap()));
    assert!(bundle.concepts()[0].is_stale_on(Date::new(2026, 12, 31).unwrap()));

    // Nothing here is malformed.
    assert!(
        !report
            .of(Severity::Warning)
            .any(|d| d.message.contains("is not an RFC 3339 date-time")),
        "{:#?}",
        report.diagnostics
    );
    // The two timestamps consumers compare should be date-times, though.
    let prefer: Vec<_> = report
        .of(Severity::Warning)
        .filter(|d| d.message.contains("write it as a date-time"))
        .collect();
    assert_eq!(prefer.len(), 2, "{:#?}", report.diagnostics);
    assert!(prefer.iter().all(|d| d.fixable));
    assert!(prefer.iter().any(|d| d.message.contains("`generated.at`")));
    assert!(
        prefer
            .iter()
            .any(|d| d.message.contains("`verified[0].at`"))
    );

    // A date-time without an offset stays invalid.
    assert!(okf_core::DateTime::parse("2026-06-26T10:00:00").is_some());
    assert!(okf_core::DateTime::parse_rfc3339("2026-06-26T10:00:00").is_none());
    assert!(!okf_validator::validate::is_iso8601_datetime("2026-06-26"));
    assert!(!okf_validator::validate::is_iso8601_datetime(
        "2026-06-26T00:00:00"
    ));
    assert!(okf_validator::validate::is_iso8601_datetime(
        "2026-06-26T00:00:00Z"
    ));
}

#[test]
fn malformed_and_unreadable_reserved_files_are_conformance_errors() {
    let tmp = TempDir::new();
    tmp.write("a.md", "---\ntype: Note\n---\nBody.\n");
    tmp.write("index.md", "---\nokf_version: 0.3\n");
    tmp.write("nested/index.md", "---\ntype: Not allowed\n---\n");
    tmp.write(
        "log.md",
        "# Log\n\n## someday\n* **Update**: broken date.\n",
    );

    let bundle = Bundle::load(tmp.path()).unwrap();
    let report = validate_bundle(&bundle);
    assert!(!report.is_conformant());
    assert!(report.of(Severity::Error).any(|d| {
        d.path.as_ref().is_some_and(|p| p.ends_with("index.md"))
            && d.message.contains("does not parse as a YAML mapping")
    }));
    assert!(report.of(Severity::Error).any(|d| {
        d.message
            .contains("an index.md below the bundle root has no frontmatter")
    }));
    assert!(
        report
            .of(Severity::Error)
            .any(|d| { d.message.contains("log date heading is not ISO-8601") })
    );
    // A reserved name is never a concept, whatever frontmatter it contains.
    assert_eq!(bundle.len(), 1);

    let unreadable = TempDir::new();
    unreadable.write("a.md", "---\ntype: Note\n---\nBody.\n");
    std::fs::write(unreadable.path().join("index.md"), [0xff, 0xfe]).unwrap();
    std::fs::write(unreadable.path().join("log.md"), [0xff, 0xfe]).unwrap();
    let bundle = Bundle::load(unreadable.path()).unwrap();
    let report = validate_bundle(&bundle);
    assert!(!report.is_conformant());
    assert!(
        report
            .of(Severity::Error)
            .any(|d| d.message.contains("unreadable reserved index.md"))
    );
    assert!(
        report
            .of(Severity::Error)
            .any(|d| d.message.contains("unreadable reserved log.md"))
    );
}

#[test]
fn the_root_index_carries_bundle_metadata() {
    // The bundle-root index.md may have frontmatter: the OKF version, a
    // title, a description, a license, and extension keys.
    let tmp = TempDir::new();
    tmp.write("a.md", "---\ntype: Note\n---\nBody.\n");
    tmp.write("LICENSE.txt", "All rights reserved.\n");
    tmp.write(
        "index.md",
        "---\nokf_version: \"0.3\"\ntitle: Acme\ndescription: Notes. Use it for nothing.\n\
         license: /LICENSE.txt\nx-acme-owner: data-team\n---\n\n# Note\n\n* [a](a.md)\n",
    );
    let bundle = Bundle::load(tmp.path()).unwrap();
    let report = validate_bundle(&bundle);
    assert!(report.is_conformant(), "{:#?}", report.diagnostics);
    assert!(
        !report
            .diagnostics
            .iter()
            .any(|d| d.path.as_ref().is_some_and(|p| p.ends_with("index.md"))),
        "{:#?}",
        report.diagnostics
    );
    assert_eq!(bundle.metadata().license.as_deref(), Some("/LICENSE.txt"));

    // A license that is neither an SPDX expression nor a file is worth a word.
    tmp.write(
        "index.md",
        "---\nokf_version: \"0.3\"\nlicense: see the wiki for terms\n---\n\n# Note\n\n* [a](a.md)\n",
    );
    let report = validate_bundle(&Bundle::load(tmp.path()).unwrap());
    assert!(report.is_conformant());
    assert!(report.of(Severity::Warning).any(|d| {
        d.message
            .contains("`license` is neither an SPDX expression nor a file")
    }));
}

#[test]
fn a_bundle_should_declare_its_version() {
    // No root index at all.
    let tmp = TempDir::new();
    tmp.write("a.md", "---\ntype: Note\n---\nBody.\n");
    let report = validate_bundle(&Bundle::load(tmp.path()).unwrap());
    assert!(
        report.is_conformant(),
        "a missing index.md is never a violation"
    );
    assert!(
        report
            .of(Severity::Warning)
            .any(|d| d.message.contains("no bundle-root index.md"))
    );

    // A root index with no frontmatter, or frontmatter without the key.
    for index in [
        "# Note\n\n* [a](a.md)\n",
        "---\ntitle: Acme\n---\n\n# Note\n\n* [a](a.md)\n",
    ] {
        tmp.write("index.md", index);
        let report = validate_bundle(&Bundle::load(tmp.path()).unwrap());
        assert!(report.is_conformant());
        assert!(
            report
                .of(Severity::Warning)
                .any(|d| d.message.contains("does not declare `okf_version`") && d.fixable),
            "{:#?}",
            report.diagnostics
        );
    }

    tmp.write(
        "index.md",
        "---\nokf_version: \"0.3\"\n---\n\n# Note\n\n* [a](a.md)\n",
    );
    let report = validate_bundle(&Bundle::load(tmp.path()).unwrap());
    assert!(
        !report
            .diagnostics
            .iter()
            .any(|d| d.message.contains("okf_version"))
    );
}

#[test]
fn log_structure_beyond_dates_and_frontmatter_is_a_warning() {
    // A log is non-conformant for frontmatter or for a date heading that is
    // not `YYYY-MM-DD`. Anything else about its shape still reads, so it is
    // worth a warning and no more.
    for (contents, expected) in [
        (
            "# My Update History\n\nThis is not a log.\n",
            "no date groups",
        ),
        ("# My Update History\n\n## 2026-06-26\n", "has no entries"),
        (
            "# My Update History\n\n## 2026-06-27\n* ordinary prose entry\nUnexpected prose.\n",
            "non-log content",
        ),
        (
            "# History\n\nAn introduction to this log.\n\n## 2026-06-27\n\n- an entry\n",
            "non-log content",
        ),
    ] {
        let tmp = TempDir::new();
        tmp.write("a.md", "---\ntype: Note\n---\nBody.\n");
        tmp.write("log.md", contents);
        let report = validate_bundle(&Bundle::load(tmp.path()).unwrap());

        assert!(report.is_conformant(), "rejected log: {contents:?}");
        assert!(
            report
                .of(Severity::Warning)
                .any(|diagnostic| { diagnostic.message.contains(expected) }),
            "{expected:?}: {:#?}",
            report.diagnostics
        );
    }
}

#[test]
fn a_log_that_is_not_newest_first_is_a_warning() {
    let tmp = TempDir::new();
    tmp.write("a.md", "---\ntype: Note\n---\nBody.\n");
    tmp.write(
        "log.md",
        "# History\n\n## 2026-06-26\n* an entry\n\n## 2026-06-27\n* a newer entry\n",
    );
    let report = validate_bundle(&Bundle::load(tmp.path()).unwrap());
    assert!(report.is_conformant(), "{:#?}", report.diagnostics);
    assert!(report.of(Severity::Warning).any(|d| {
        d.message
            .contains("lists the newest date first: \"2026-06-27\" follows \"2026-06-26\"")
    }));
}

#[test]
fn valid_log_structure_allows_custom_titles_and_unmarked_prose_entries() {
    let tmp = TempDir::new();
    tmp.write("a.md", "---\ntype: Note\n---\nBody.\n");
    tmp.write(
        "log.md",
        "# Whatever the producer calls this\n\n\
         ## 2026-06-27\n\
         * A plain prose entry with no bold kind marker.\n\
         * - Another ordinary entry.\n\
         \x20 Continued prose is still part of the preceding entry.\n\
         ## 2026-06-26\n\
         - **Update**: An older marked entry.\n",
    );
    let report = validate_bundle(&Bundle::load(tmp.path()).unwrap());

    assert!(report.is_conformant(), "{:#?}", report.diagnostics);
}

#[test]
fn log_with_frontmatter_is_a_conformance_error() {
    // Earlier versions were silent on this; a log.md now has no frontmatter.
    let tmp = TempDir::new();
    tmp.write("a.md", "---\ntype: Note\n---\nBody.\n");
    tmp.write(
        "log.md",
        "---\ntype: Log\ntitle: Bundle history\n---\n\n\
         # Bundle history\n\n\
         ## 2026-07-01\n\
         - **Verified** the full bundle.\n",
    );
    let bundle = Bundle::load(tmp.path()).unwrap();
    // It is still not a concept, and its entries still read.
    assert_eq!(bundle.len(), 1);
    let report = validate_bundle(&bundle);

    assert!(!report.is_conformant());
    let errors: Vec<_> = report.of(Severity::Error).collect();
    assert_eq!(errors.len(), 1, "{errors:#?}");
    assert!(errors[0].message.contains("log.md has no frontmatter"));
    assert!(errors[0].fixable);
}

#[test]
fn declared_version_is_reported_never_enforced() {
    for (declared, needle) in [
        ("0.1", "declares OKF v0.1"),
        ("0.2", "declares OKF v0.2"),
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
            "a version mismatch is never a violation"
        );
        assert!(
            report
                .of(Severity::Info)
                .any(|d| d.message.contains(needle)),
            "{declared}: {:#?}",
            report.diagnostics
        );
    }

    // The fallbacks for earlier versions apply to a bundle that declares 0.2
    // or earlier, or declares nothing.
    for (declared, applies) in [
        (None, true),
        (Some("0.1"), true),
        (Some("0.2"), true),
        (Some("0.3"), false),
        (Some("1.0"), false),
        (Some("0.2.1"), false),
        (Some("not-a-version"), true),
    ] {
        assert_eq!(
            okf_core::legacy_fallbacks_apply(declared),
            applies,
            "{declared:?}"
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
    assert!(report.of(Severity::Info).any(|d| {
        d.message
            .contains("a sanctioned computation is its own concept")
    }));
}

#[test]
fn every_frontmatter_family_round_trips() {
    let doc = Document::parse(REVENUE).unwrap();
    // Nothing was changed, so nothing is re-serialized: the spec's own text
    // comes back byte for byte, flow mappings and quoted timestamps included.
    assert_eq!(doc.serialize(), REVENUE);

    // The canonical form re-emits flow mappings as block mappings, which is
    // the same value written differently.
    let reparsed = Document::parse(&doc.serialize_canonical()).unwrap();
    assert_eq!(reparsed.frontmatter, doc.frontmatter);
    let contract = reparsed.attested_computation().unwrap();
    assert_eq!(contract.runtime.as_deref(), Some("bigquery"));
    assert_eq!(reparsed.frontmatter.trust_tier(), TrustTier::HumanReviewed);
    assert_eq!(reparsed.frontmatter.sources().len(), 2);
    assert!(reparsed.frontmatter.extension_keys().is_empty());
}

#[test]
fn regenerating_indexes_keeps_the_bundle_metadata() {
    let tmp = finance_bundle();
    okf_core::index::regenerate_indexes(tmp.path()).unwrap();

    // Everything up to the listing is the original frontmatter, untouched.
    let root = tmp.read("index.md");
    let frontmatter_end = INDEX.find("# Metrics").unwrap();
    assert!(root.starts_with(&INDEX[..frontmatter_end]), "{root}");
    assert!(root.contains("# Subdirectories"), "{root}");
    assert!(!root.contains("# Metrics"), "{root}");

    let bundle = Bundle::load(tmp.path()).unwrap();
    assert_eq!(bundle.okf_version(), Some("0.3"));
    assert_eq!(bundle.metadata().title.as_deref(), Some("Acme finance"));

    // Attested Computations get their own index section, which is how
    // discovery-by-type works from an index.
    let computations = tmp.read("computations/index.md");
    assert!(
        computations.starts_with("# Attested Computation"),
        "{computations}"
    );
    // `references/` holds no concepts, so it has nothing to list.
    assert!(!tmp.path().join("references/index.md").exists());
    assert!(!root.contains("references"), "{root}");
}

#[test]
fn test_validate_notes_when_verified_predates_generated() {
    let tmp = TempDir::new();
    tmp.write("index.md", "# Test\n\n* [Metric](metric.md)\n");
    tmp.write(
        "metric.md",
        "---\ntype: Metric\ntitle: Revenue\ndescription: d\n\
         generated: { by: ref/x, at: 2026-06-28T14:00:00Z }\n\
         verified: { by: human:a, at: 2026-06-25T09:00:00Z }\n\
         ---\n\n# Definition\n\nProse.\n",
    );
    let bundle = Bundle::load(tmp.path()).unwrap();
    let report = validate_bundle(&bundle);
    // Content can change without a new confirmation: a permitted state, and
    // one a producer wants to know about.
    let notes: Vec<_> = report
        .of(Severity::Info)
        .filter(|d| d.message.contains("no verification is current"))
        .collect();
    assert_eq!(
        notes.len(),
        1,
        "expected a note for verified predating generated: {:#?}",
        report.diagnostics
    );
    assert_eq!(bundle.concepts()[0].trust_tier(), TrustTier::Unverified);
}

#[test]
fn test_validate_warns_on_future_timestamp() {
    let tmp = TempDir::new();
    tmp.write("index.md", "# Test\n\n* [Metric](metric.md)\n");
    tmp.write(
        "metric.md",
        "---\ntype: Metric\ntitle: Revenue\ndescription: d\n\
         generated: { by: ref/x, at: 2099-01-01T00:00:00Z }\n\
         verified: { by: human:a, at: 2026-06-25T09:00:00Z }\n\
         ---\n\n# Definition\n\nProse.\n",
    );
    let bundle = Bundle::load(tmp.path()).unwrap();
    let today = Date::new(2026, 7, 1).unwrap();
    let report = validate_bundle_at(&bundle, Some(today));
    let warnings: Vec<_> = report
        .of(Severity::Warning)
        .filter(|d| d.message.contains("is in the future"))
        .collect();
    assert_eq!(
        warnings.len(),
        1,
        "expected warning for future timestamp: {warnings:#?}"
    );
}

#[test]
fn test_validate_warns_on_empty_body() {
    let tmp = TempDir::new();
    tmp.write("index.md", "# Test\n\n* [Metric](metric.md)\n");
    tmp.write(
        "metric.md",
        "---\ntype: Metric\ntitle: Revenue\ndescription: d\n\
         generated: { by: ref/x, at: 2026-06-20T22:53:05Z }\n\
         ---\n\n   \n",
    );
    let bundle = Bundle::load(tmp.path()).unwrap();
    let report = validate_bundle(&bundle);
    let warnings: Vec<_> = report
        .of(Severity::Warning)
        .filter(|d| d.message.contains("body is empty"))
        .collect();
    assert_eq!(
        warnings.len(),
        1,
        "expected warning for empty body: {warnings:#?}"
    );
}

#[test]
fn test_validate_warns_on_circular_derivation() {
    let tmp = TempDir::new();
    tmp.write("index.md", "# Test\n\n* [A](a.md)\n* [B](b.md)\n");
    tmp.write(
        "a.md",
        "---\ntype: Metric\ntitle: A\ndescription: d\n\
         generated: { by: ref/x, at: 2026-06-20T22:53:05Z }\n\
         sources:\n  - { resource: b.md }\n\
         ---\n\n# Definition\n\nSee [B](b.md).\n",
    );
    tmp.write(
        "b.md",
        "---\ntype: Metric\ntitle: B\ndescription: d\n\
         generated: { by: ref/x, at: 2026-06-20T22:53:05Z }\n\
         sources:\n  - { resource: a.md }\n\
         ---\n\n# Definition\n\nSee [A](a.md).\n",
    );
    let bundle = Bundle::load(tmp.path()).unwrap();
    let report = validate_bundle(&bundle);
    let warnings: Vec<_> = report
        .of(Severity::Warning)
        .filter(|d| d.message.contains("circular concept derivation"))
        .collect();
    assert!(
        !warnings.is_empty(),
        "expected warning for circular derivation: {warnings:#?}"
    );
}

#[test]
fn test_validate_warns_on_path_fields_that_do_not_resolve() {
    let tmp = TempDir::new();
    tmp.write("index.md", "# Test\n\n* [Calc](comp.md)\n");
    tmp.write("lib/present.sql", "SELECT 1\n");
    tmp.write(
        "comp.md",
        "---\ntype: Attested Computation\ntitle: Calc\ndescription: d\n\
         generated: { by: ref/x, at: 2026-06-20T22:53:05Z }\n\
         runtime: python\n\
         parameters:\n  - { name: x, type: string }\n\
         computation: /lib/present.sql\n\
         executor:\n  resource: non_existent_script.py\n  receipt: [res]\n\
         attester:\n  resource: https://example.com/verifier.py\n\
         sources:\n\
         \x20 - { resource: /policies/missing.md }\n\
         \x20 - { resource: all queries in project X }\n\
         resource: acme.sales.orders\n\
         ---\n\n# Calc\n\nProse.\n",
    );
    let bundle = Bundle::load(tmp.path()).unwrap();
    let report = validate_bundle(&bundle);
    let unresolved: Vec<&str> = report
        .of(Severity::Warning)
        .filter(|d| {
            d.message
                .contains("does not resolve to a file in the bundle")
        })
        .map(|d| d.message.as_str())
        .collect();
    // A URL, a scope descriptor, and an opaque `resource` identifier are not
    // paths into the bundle; the computation file is there.
    assert_eq!(unresolved.len(), 2, "{unresolved:#?}");
    assert!(unresolved.iter().any(|m| m.contains("`executor.resource`")));
    assert!(
        unresolved
            .iter()
            .any(|m| m.contains("`sources[0].resource`"))
    );
    assert!(bundle.is_attestable(&id("comp")));

    // An attester that is not there cannot be run.
    tmp.write(
        "comp.md",
        "---\ntype: Attested Computation\nruntime: python\ncomputation: /lib/present.sql\n\
         attester:\n  resource: /lib/absent.py\n---\n\nProse.\n",
    );
    assert!(!Bundle::load(tmp.path()).unwrap().is_attestable(&id("comp")));

    // A `computation` that names a missing file makes the concept not
    // attestable.
    tmp.write(
        "comp.md",
        "---\ntype: Attested Computation\nruntime: python\ncomputation: /lib/absent.sql\n\
         attester:\n  resource: /lib/present.sql\n---\n\nProse.\n",
    );
    let bundle = Bundle::load(tmp.path()).unwrap();
    assert!(!bundle.is_attestable(&id("comp")));
    assert!(
        validate_bundle(&bundle)
            .of(Severity::Warning)
            .any(|d| d.message.contains("`computation` does not resolve"))
    );
}

#[test]
fn test_validate_reports_broken_cross_link_info() {
    let tmp = TempDir::new();
    tmp.write("index.md", "# Test\n\n* [A](a.md)\n");
    tmp.write(
        "a.md",
        "---\ntype: Metric\ntitle: A\ndescription: d\n\
         generated: { by: ref/x, at: 2026-06-20T22:53:05Z }\n\
         ---\n\n# Definition\n\nLink to [nonexistent](ghost.md).\n",
    );
    let bundle = Bundle::load(tmp.path()).unwrap();
    let report = validate_bundle(&bundle);
    let infos: Vec<_> = report
        .of(Severity::Info)
        .filter(|d| d.message.contains("ghost.md"))
        .collect();
    assert_eq!(infos.len(), 1, "expected info for broken link: {infos:#?}");
}

#[test]
fn a_link_to_an_asset_or_a_non_concept_file_is_not_broken() {
    let tmp = TempDir::new();
    tmp.write(
        "index.md",
        "---\nokf_version: \"0.3\"\n---\n\n# Metric\n\n* [A](a.md)\n",
    );
    tmp.write("README.md", "# Read me\n");
    tmp.write("img/chart.png", "not really a png");
    tmp.write("attesters/check.py", "print(1)\n");
    tmp.write(
        "a.md",
        "---\ntype: Metric\ntitle: A\ndescription: d\n\
         generated: { by: ref/x, at: 2026-06-20T22:53:05Z }\n\
         ---\n\n# Definition\n\n\
         See the [chart](img/chart.png), the [attester](/attesters/check.py), the\n\
         [readme](README.md), the [listing](index.md), and a [ghost](ghost.md).\n",
    );
    let bundle = Bundle::load(tmp.path()).unwrap();
    let kinds: Vec<(&str, LinkTarget)> = bundle
        .links_from(&id("a"))
        .iter()
        .map(|link| (link.raw.as_str(), link.resolution))
        .collect();
    assert_eq!(
        kinds,
        vec![
            ("img/chart.png", LinkTarget::File),
            ("/attesters/check.py", LinkTarget::File),
            ("README.md", LinkTarget::File),
            ("index.md", LinkTarget::File),
            ("ghost.md", LinkTarget::Missing),
        ]
    );
    // Only the last one names nothing.
    assert_eq!(
        bundle.broken_links(),
        vec![(id("a"), "ghost.md".to_string())]
    );
    let report = validate_bundle(&bundle);
    assert_eq!(
        report
            .of(Severity::Info)
            .filter(|d| d.message.contains("link target is not in the bundle"))
            .count(),
        1
    );
}

#[test]
fn test_validate_warns_on_link_to_deprecated_concept() {
    let tmp = TempDir::new();
    tmp.write(
        "index.md",
        "# Test\n\n* [New](new.md)\n* [Old](old.md)\n* [Other](other.md)\n",
    );
    tmp.write(
        "old.md",
        "---\ntype: Metric\ntitle: Old\ndescription: d\nstatus: deprecated\n\
         superseded_by: /new.md\n\
         generated: { by: ref/x, at: 2026-06-20T22:53:05Z }\n\
         ---\n\n# Definition\n\nSee [the new one](new.md).\n",
    );
    tmp.write(
        "new.md",
        "---\ntype: Metric\ntitle: New\ndescription: d\n\
         generated: { by: ref/x, at: 2026-06-20T22:53:05Z }\n\
         ---\n\n# Definition\n\nSee [the old one](old.md).\n",
    );
    tmp.write(
        "other.md",
        "---\ntype: Metric\ntitle: Other\ndescription: d\n\
         generated: { by: ref/x, at: 2026-06-20T22:53:05Z }\n\
         ---\n\n# Definition\n\nUses [the old one](old.md).\n",
    );
    let bundle = Bundle::load(tmp.path()).unwrap();
    let report = validate_bundle(&bundle);
    let warnings: Vec<&str> = report
        .of(Severity::Warning)
        .filter(|d| d.message.contains("links to deprecated concept `old`"))
        .map(|d| d.message.as_str())
        .collect();
    assert_eq!(
        warnings.len(),
        2,
        "expected warnings for linking to deprecated concept: {warnings:#?}"
    );
    // A concept that links to a retired one is told where to go instead.
    assert!(
        warnings
            .iter()
            .any(|w| w.ends_with("it is superseded by `new`"))
    );
}

#[test]
fn superseded_by_leads_from_a_retired_concept_to_its_replacement() {
    let tmp = TempDir::new();
    let concept = |status: &str, extra: &str| {
        format!("---\ntype: Metric\nstatus: {status}\n{extra}---\n\nBody.\n")
    };
    tmp.write("metrics/gross-margin.md", &concept("stable", ""));
    tmp.write(
        "metrics/gross-margin-legacy.md",
        &concept("deprecated", "superseded_by: /metrics/gross-margin.md\n"),
    );
    // Relative works too, and a chain is followed to its end.
    tmp.write(
        "metrics/gm-2019.md",
        &concept(
            "deprecated",
            "superseded_by: gross-margin-legacy.md#definition\n",
        ),
    );
    // The field has a meaning only with `status: deprecated`.
    tmp.write(
        "metrics/revenue.md",
        &concept("stable", "superseded_by: gross-margin.md\n"),
    );
    // Retired with no replacement, and a loop.
    tmp.write("metrics/retired.md", &concept("deprecated", ""));
    tmp.write(
        "metrics/a.md",
        &concept("deprecated", "superseded_by: b.md\n"),
    );
    tmp.write(
        "metrics/b.md",
        &concept("deprecated", "superseded_by: a.md\n"),
    );
    let bundle = Bundle::load(tmp.path()).unwrap();

    let current = id("metrics/gross-margin");
    let legacy = id("metrics/gross-margin-legacy");
    assert_eq!(bundle.superseded_by(&legacy), Some(&current));
    assert_eq!(bundle.superseded_by(&id("metrics/gm-2019")), Some(&legacy));
    assert_eq!(bundle.superseded_by(&id("metrics/revenue")), None);
    assert_eq!(bundle.superseded_by(&id("metrics/retired")), None);

    let replacement = |name: &str| {
        bundle
            .current_replacement(&id(name))
            .map(|concept| concept.id.to_string())
    };
    assert_eq!(
        replacement("metrics/gm-2019").as_deref(),
        Some("metrics/gross-margin")
    );
    assert_eq!(
        replacement("metrics/gross-margin").as_deref(),
        Some("metrics/gross-margin")
    );
    assert_eq!(replacement("metrics/retired"), None);
    assert_eq!(replacement("metrics/a"), None, "a loop has no current end");

    // The field is an edge of the bundle graph, like a link.
    assert_eq!(bundle.path_edges_from(&legacy).len(), 1);
    assert_eq!(bundle.path_edges_from(&legacy)[0].field, "superseded_by");
    assert!(bundle.referenced_by(&current).contains(&legacy));
    assert!(bundle.backlinks(&current).is_empty());

    let report = validate_bundle(&bundle);
    assert!(report.is_conformant());
    let misuse: Vec<_> = report
        .of(Severity::Warning)
        .filter(|d| {
            d.message
                .contains("has a meaning only with `status: deprecated`")
        })
        .collect();
    assert_eq!(misuse.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(misuse[0].concept, Some(id("metrics/revenue")));
}

#[test]
fn path_fields_that_name_concepts_are_graph_edges() {
    let tmp = finance_bundle();
    tmp.write(
        "references/skills/run-on-bq.md",
        "---\ntype: Skill\ntitle: Run on BigQuery\n---\n\nRun instructions.\n",
    );
    let bundle = Bundle::load(tmp.path()).unwrap();
    let revenue = id("computations/revenue");
    let skill = id("references/skills/run-on-bq");

    // The executor is now a concept, so the field is an edge. The attester is
    // an asset, and the sources are URLs: no edges there.
    let edges = bundle.path_edges_from(&revenue);
    assert_eq!(edges.len(), 1, "{edges:#?}");
    assert_eq!(edges[0].field, "executor.resource");
    assert_eq!(edges[0].target, skill);
    assert!(!edges[0].is_source());
    assert_eq!(bundle.referenced_by(&skill), std::slice::from_ref(&revenue));
    // It is not a derivation edge, and not a link.
    assert!(bundle.derived_from(&revenue).is_empty());
    assert!(bundle.backlinks(&skill).is_empty());
}

#[test]
fn test_validate_warns_on_duplicate_concept_titles() {
    let tmp = TempDir::new();
    tmp.write("index.md", "# Test\n\n* [a](a.md)\n* [b](b.md)\n");
    for name in ["a.md", "b.md"] {
        tmp.write(
            name,
            "---\ntype: Metric\ntitle: Same title\ndescription: d\n\
             generated: { by: ref/x, at: 2026-06-20T22:53:05Z }\n\
             ---\n\n# Definition\n\nProse.\n",
        );
    }
    let bundle = Bundle::load(tmp.path()).unwrap();
    let report = validate_bundle(&bundle);
    let warnings: Vec<_> = report
        .of(Severity::Warning)
        .filter(|d| {
            d.message
                .contains("`title` \"Same title\" is shared with another concept")
        })
        .collect();
    assert_eq!(
        warnings.len(),
        2,
        "expected 2 warnings for duplicate titles: {warnings:#?}"
    );
}

#[test]
fn test_validate_warns_on_duplicate_log_dates() {
    let tmp = TempDir::new();
    tmp.write("index.md", "# Test\n\n* [Revenue](metric.md)\n");
    tmp.write(
        "metric.md",
        "---\ntype: Metric\ntitle: Revenue\ndescription: d\n\
         generated: { by: ref/x, at: 2026-06-20T22:53:05Z }\n\
         ---\n\n# Definition\n\nProse.\n",
    );
    tmp.write(
        "log.md",
        "# Update Log\n\n## 2026-05-22\n* **Update**: First.\n\n## 2026-05-22\n* **Update**: Duplicate date.\n",
    );
    let bundle = Bundle::load(tmp.path()).unwrap();
    let report = validate_bundle(&bundle);
    let warnings: Vec<_> = report
        .of(Severity::Warning)
        .filter(|d| d.message.contains("duplicate date heading `## 2026-05-22`"))
        .collect();
    assert_eq!(
        warnings.len(),
        1,
        "expected warning for duplicate log date: {warnings:#?}"
    );
    assert!(
        warnings[0].fixable,
        "duplicate log date warning should be fixable"
    );
}

#[test]
fn test_validate_warns_on_stale_index() {
    let tmp = TempDir::new();
    tmp.write("index.md", "# Test\n\n* [A](a.md)\n");
    tmp.write(
        "a.md",
        "---\ntype: Metric\ntitle: A\ndescription: d\n\
         generated: { by: ref/x, at: 2026-06-20T22:53:05Z }\n\
         ---\n\n# Definition\n\nProse.\n",
    );
    tmp.write(
        "b.md",
        "---\ntype: Metric\ntitle: B\ndescription: d\n\
         generated: { by: ref/x, at: 2026-06-20T22:53:05Z }\n\
         ---\n\n# Definition\n\nProse.\n",
    );
    let bundle = Bundle::load(tmp.path()).unwrap();
    let report = validate_bundle(&bundle);
    let warnings: Vec<_> = report
        .of(Severity::Warning)
        .filter(|d| {
            d.message
                .contains("index.md is out of sync with its directory")
        })
        .collect();
    assert_eq!(
        warnings.len(),
        1,
        "expected warning for stale index: {warnings:#?}"
    );
    assert!(warnings[0].fixable, "stale index warning should be fixable");
}

#[test]
fn test_validate_warns_on_broken_link_anchors() {
    let tmp = common::TempDir::new();
    tmp.write(
        "index.md",
        "---\nokf_version: \"0.3\"\n---\n\n# Test\n\n* [Pricing](pricing.md)\n* [Overview](overview.md)\n",
    );
    tmp.write(
        "pricing.md",
        "---\ntype: Concept\ntitle: Pricing\ngenerated:\n  by: human:alice\n  at: 2026-01-01T00:00:00Z\n---\n\n# Pricing\n\n## Valid Section\nContent.\n",
    );
    tmp.write(
        "overview.md",
        "---\ntype: Concept\ntitle: Overview\ngenerated:\n  by: human:alice\n  at: 2026-01-01T00:00:00Z\n---\n\n# Overview\n\nSee [Good](pricing.md#valid-section) and [Bad](pricing.md#non-existent-section).\nAlso see [Bad Internal](#broken-internal).\n",
    );

    let bundle = Bundle::load(tmp.path()).unwrap();
    let report = validate_bundle(&bundle);

    let warnings: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.severity == okf_validator::Severity::Warning)
        .collect();

    assert!(warnings.iter().any(|d| d.message.contains(
        "link anchor `#non-existent-section` does not match any heading in target concept `pricing`"
    )));
    assert!(warnings.iter().any(|d| d.message.contains(
        "internal link anchor `#broken-internal` does not match any heading in this document"
    )));
}

#[test]
fn test_validate_accepts_url_encoded_link_anchors() {
    let tmp = common::TempDir::new();
    tmp.write(
        "index.md",
        "---\nokf_version: \"0.3\"\n---\n\n# Test\n\n* [Pricing](pricing.md)\n* [Overview](overview.md)\n",
    );
    tmp.write(
        "pricing.md",
        "---\ntype: Concept\ntitle: Pricing\ngenerated:\n  by: human:alice\n  at: 2026-01-01T00:00:00Z\n---\n\n# Pricing\n\n## Valid Section\nContent.\n",
    );
    tmp.write(
        "overview.md",
        "---\ntype: Concept\ntitle: Overview\ngenerated:\n  by: human:alice\n  at: 2026-01-01T00:00:00Z\n---\n\n# Overview\n\nSee [Good](pricing.md#valid%20section) and [Internal](#internal%20heading).\n\n## Internal Heading\nInternal.\n",
    );

    let bundle = Bundle::load(tmp.path()).unwrap();
    let report = validate_bundle(&bundle);

    let anchor_warnings: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.severity == okf_validator::Severity::Warning && d.message.contains("anchor"))
        .collect();

    assert!(
        anchor_warnings.is_empty(),
        "expected 0 anchor warnings, got: {anchor_warnings:#?}"
    );
}

#[test]
fn test_validate_date_formats_fixable_and_unfixable() {
    let tmp = common::TempDir::new();
    tmp.write(
        "index.md",
        "---\nokf_version: \"0.3\"\n---\n\n# Test\n\n* [Fixable](fixable.md)\n* [Unfixable](unfixable.md)\n* [Valid](valid.md)\n",
    );
    tmp.write(
        "fixable.md",
        "---\ntype: Concept\ntitle: Fixable Dates\ndescription: Fixable dates concept.\ngenerated:\n  by: human:alice\n  at: '2026-06-30'\nverified:\n  - by: human:bob\n    at: '2026-07-01 12:00:00'\nstale_after: '2026-12-31T00:00:00'\nusage_window:\n  from: '2026-01-01T00:00'\n  to: '2026-06-01 00:00:00+0000'\nsources:\n  - id: '1'\n    resource: https://example.com\n    last_modified: '2026-05-15T10:00:00'\n---\n\n# Fixable Dates\n\nBody.[^1]\n\n[^1]: Example\n",
    );
    tmp.write(
        "unfixable.md",
        "---\ntype: Concept\ntitle: Unfixable Dates\ndescription: Unfixable dates concept.\ngenerated:\n  by: human:alice\n  at: not-a-date\nstale_after: last-tuesday\n---\n\n# Unfixable Dates\n\nBody.\n",
    );
    // Date-only values are valid where no comparison is made.
    tmp.write(
        "valid.md",
        "---\ntype: Concept\ntitle: Valid Dates\ndescription: Valid dates concept.\ngenerated:\n  by: human:alice\n  at: '2026-06-30T00:00:00Z'\nstale_after: '2026-12-31'\nusage_window:\n  from: '2026-01-01'\n  to: '2026-06-01'\nsources:\n  - id: '1'\n    resource: https://example.com\n    last_modified: '2026-05-15'\n---\n\n# Valid Dates\n\nBody.[^1]\n\n[^1]: Example\n",
    );

    let bundle = Bundle::load(tmp.path()).unwrap();
    let report = validate_bundle(&bundle);
    let on = |name: &str| -> Vec<&okf_validator::Diagnostic> {
        report
            .diagnostics
            .iter()
            .filter(|d| d.path.as_ref().is_some_and(|p| p.ends_with(name)))
            .collect()
    };

    let fixable_diag = on("fixable.md");
    assert_eq!(fixable_diag.len(), 6, "{fixable_diag:#?}");
    for d in &fixable_diag {
        assert!(
            d.fixable,
            "diagnostic on fixable.md should be fixable: {}",
            d.message
        );
    }

    let unfixable_diag = on("unfixable.md");
    assert!(!unfixable_diag.is_empty());
    for d in &unfixable_diag {
        assert!(
            !d.fixable,
            "diagnostic on unfixable.md should NOT be fixable: {}",
            d.message
        );
    }

    assert!(on("valid.md").is_empty(), "{:#?}", on("valid.md"));
}

#[test]
fn staleness_is_judged_at_an_instant() {
    use okf_validator::validate_bundle_at_instant;
    let tmp = TempDir::new();
    tmp.write(
        "a.md",
        "---\ntype: Note\nstale_after: \"2026-07-01T10:00:00Z\"\n---\nBody.\n",
    );
    let bundle = Bundle::load(tmp.path()).unwrap();
    let stale_at = |instant: &str| {
        validate_bundle_at_instant(&bundle, okf_core::DateTime::parse_rfc3339(instant))
            .of(Severity::Info)
            .any(|d| d.message.contains("stale since"))
    };
    // Two hours after `stale_after`, on the same day: stale.
    assert!(stale_at("2026-07-01T12:00:00Z"));
    assert!(stale_at("2026-07-01T10:00:00Z"));
    assert!(!stale_at("2026-07-01T09:59:59Z"));
    // A pinned day is judged at its start.
    assert!(
        !validate_bundle_at(&bundle, Date::new(2026, 7, 1))
            .of(Severity::Info)
            .any(|d| d.message.contains("stale since"))
    );
    assert!(
        validate_bundle_at(&bundle, Date::new(2026, 7, 2))
            .of(Severity::Info)
            .any(|d| d.message.contains("stale since"))
    );
}

#[test]
fn values_of_the_wrong_shape_are_not_read_as_absent() {
    // A `stale_after` that cannot be parsed is never fresh, whatever its
    // shape, and a `status` that is not one of the three values is not
    // stable.
    let tmp = TempDir::new();
    tmp.write(
        "a.md",
        "---\ntype: Note\nstale_after: [\"2020-01-01\"]\nstatus: [deprecated]\n\
         verified: { by: \"human:Jane Doe\", at: \"2026-06-25T09:00:00Z\" }\n---\nBody.\n",
    );
    let bundle = Bundle::load(tmp.path()).unwrap();
    let concept = &bundle.concepts()[0];
    let now = Date::new(2019, 1, 1).unwrap().to_utc_datetime();
    assert_eq!(concept.freshness_at(now), Freshness::Malformed);
    assert!(concept.is_stale_at(now));
    assert!(!concept.is_current());
    assert!(!concept.status().is_known());
    // The id of an actor is opaque: a person with a space in the name is a
    // person, though the spelling is worth a warning.
    assert_eq!(concept.trust_tier(), TrustTier::HumanReviewed);

    let report = validate_bundle(&bundle);
    assert!(report.is_conformant());
    let warnings: Vec<&str> = report
        .of(Severity::Warning)
        .map(|d| d.message.as_str())
        .collect();
    assert!(
        warnings
            .iter()
            .any(|w| w.contains("`stale_after` does not parse")),
        "{warnings:#?}"
    );
    assert!(
        warnings
            .iter()
            .any(|w| w.contains("unknown `status` value")),
        "{warnings:#?}"
    );
    assert!(
        warnings
            .iter()
            .any(|w| w.contains("`verified[0].by` is not an actor")),
        "{warnings:#?}"
    );
}

#[test]
fn one_file_in_another_encoding_does_not_stop_the_bundle_from_loading() {
    let tmp = TempDir::new();
    tmp.write("a.md", "---\ntype: Note\n---\nBody.\n");
    std::fs::write(
        tmp.path().join("latin1.md"),
        b"---\ntype: Note\ntitle: Caf\xe9\n---\n",
    )
    .unwrap();
    let bundle = Bundle::load(tmp.path()).unwrap();
    assert_eq!(bundle.len(), 1);
    let report = validate_bundle(&bundle);
    assert!(!report.is_conformant());
    assert!(report.of(Severity::Error).any(|d| {
        d.path.as_ref().is_some_and(|p| p.ends_with("latin1.md"))
            && d.message.contains("not valid UTF-8")
    }));
}
