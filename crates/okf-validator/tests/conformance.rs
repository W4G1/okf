//! End-to-end conformance tests for [`validate_bundle`] and conformance clauses,
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
use okf_core::{
    ActorKind, Bundle, ComputationSource, ConceptId, Date, Document, ResourceKind, Status,
    TrustTier,
};
use okf_validator::{Severity, validate_bundle, validate_bundle_at};

const INCOME_STATEMENT: &str = r"---
type: Metric
title: Income statement (fiscal year)
description: Headline income-statement figures for a fiscal year.
tags: [finance, income-statement]
status: stable
generated: { by: reference_agent/gemini-2.5-pro, at: 2026-06-20T22:53:05Z }
verified: { by: human:walter, at: 2026-06-25T09:00:00Z }
stale_after: 2026-12-31T00:00:00Z
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
verified: { by: human:walter, at: 2026-06-25T09:00:00Z }
stale_after: 2026-12-31T00:00:00Z
sources:
  - id: rev-policy
    resource: https://wiki.acme/finance/revenue-recognition
    title: Revenue recognition policy
    author: team:finance-fpa
    last_modified: 2026-04-02T00:00:00Z
  - id: exec-rev-dash
    resource: dashboards/exec-revenue
    title: Executive revenue dashboard
    author: team:finance-fpa
    usage_count: 5000
    last_modified: 2026-06-18T00:00:00Z
usage_window: { from: 2026-06-01T00:00:00Z, to: 2026-06-30T00:00:00Z }
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
stale_after: 2026-06-15T00:00:00Z
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
    assert_eq!(bundle.okf_version(), Some("0.2"));

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

    // Each computation knows which concepts use it (discovery path).
    assert_eq!(bundle.backlinks(&id("computations/revenue")), &[statement]);
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
    // `verified` is independent of `generated.at`: content changed after the
    // human sign-off, and that is legal.
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
        policy
            .last_modified
            .as_ref()
            .unwrap()
            .datetime
            .unwrap()
            .date,
        Date::new(2026, 4, 2).unwrap()
    );
    assert_eq!(policy.usage_count, None);

    // The shared window frames the dashboard's usage_count.
    let dashboard = &sources[1];
    assert_eq!(dashboard.usage_count, Some(5000));
    let shared = fm.usage_window();
    let window = dashboard.effective_usage_window(shared.as_ref()).unwrap();
    assert_eq!(
        window.from.as_ref().unwrap().datetime.unwrap().date,
        Date::new(2026, 6, 1).unwrap()
    );
    assert_eq!(
        window.to.as_ref().unwrap().datetime.unwrap().date,
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
fn contract_paths_resolve_from_the_bundle_root() {
    let tmp = finance_bundle();
    let bundle = Bundle::load(tmp.path()).unwrap();
    let revenue_id = id("computations/revenue");

    // The spec writes `references/...` from the bundle root even though the
    // concept lives in `computations/`.
    let executor = bundle
        .resolve_path_field(&revenue_id, "references/skills/run-on-bq.md")
        .unwrap();
    assert!(executor.ends_with("references/skills/run-on-bq.md"));

    let attester = bundle
        .resolve_path_field(&revenue_id, "references/attesters/sql-equality.py")
        .unwrap();
    assert!(attester.exists(), "non-markdown attester code resolves too");

    // A path-valued field resolves only to a regular file, not merely an
    // existing directory.
    assert!(
        bundle
            .resolve_path_field(&revenue_id, "references")
            .is_none()
    );

    assert!(
        bundle
            .resolve_path_field(&revenue_id, "references/nope.py")
            .is_none()
    );
    assert!(
        bundle
            .resolve_path_field(&revenue_id, "https://example.com")
            .is_none()
    );
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
        changed.datetime.unwrap().date,
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
         verified: [{ by: human:walter, at: yesterday }]\n\
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
        "`type` is present, so conformance is satisfied"
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
    has("`stale_after` is not an ISO-8601 datetime with an explicit offset");
    has("`generated.by` is required");
    has("`verified[0].at` is not an ISO-8601 datetime with an explicit offset");
    has("duplicate `sources[].id`");
    has("`sources[1].resource` is required");
    has("matches no `sources[].id`");
    has("`runtime` is required");
    has("no computation");
    has("missing `executor`");
    has("missing `attester`");
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
fn non_scalar_type_is_a_conformance_error_and_not_a_type_match() {
    let tmp = TempDir::new();
    tmp.write("bad.md", "---\ntype: [Metric, Note]\n---\nBody.\n");
    let bundle = Bundle::load(tmp.path()).unwrap();

    assert!(bundle.concepts_of_type("Metric").next().is_none());
    let report = validate_bundle(&bundle);
    assert!(!report.is_conformant());
    assert!(
        report
            .of(Severity::Error)
            .any(|d| d.message.contains("`type` must be a non-empty scalar"))
    );
}

#[test]
fn malformed_verification_events_do_not_elevate_trust_or_hide_diagnostics() {
    let tmp = TempDir::new();
    tmp.write(
        "bad.md",
        "---\n\
         type: Note\n\
         verified:\n\
         \x20 - { by: human:invalid, at: 2026-06-26 }\n\
         \x20 - { by: human:also-invalid, at: yesterday }\n\
         \x20 - not-an-event\n\
         ---\n\nBody.\n",
    );
    let bundle = Bundle::load(tmp.path()).unwrap();
    let concept = bundle.concepts().first().unwrap();
    assert_eq!(concept.trust_tier(), TrustTier::Unverified);

    let report = validate_bundle(&bundle);
    assert!(report.of(Severity::Warning).any(|d| {
        d.message
            .contains("`verified[0].at` is not an ISO-8601 datetime with an explicit offset")
    }));
    assert!(report.of(Severity::Warning).any(|d| {
        d.message
            .contains("`verified[1].at` is not an ISO-8601 datetime with an explicit offset")
    }));
    assert!(
        report
            .of(Severity::Warning)
            .any(|d| d.message.contains("`verified[2]` should be a mapping"))
    );
}

#[test]
fn trust_timestamps_require_time_but_generic_datetime_parsing_stays_permissive() {
    let tmp = TempDir::new();
    tmp.write(
        "bad.md",
        "---\n\
         type: Note\n\
         generated: { by: tool/v1, at: 2026-06-26 }\n\
         verified: { by: human:reviewer, at: 2026-06-26 }\n\
         ---\n\nBody.\n",
    );
    let bundle = Bundle::load(tmp.path()).unwrap();
    let report = validate_bundle(&bundle);

    assert!(report.is_conformant(), "{:#?}", report.diagnostics);
    assert_eq!(bundle.concepts()[0].trust_tier(), TrustTier::Unverified);
    assert!(report.of(Severity::Warning).any(|d| {
        d.message
            .contains("`generated.at` is not an ISO-8601 datetime with an explicit offset")
    }));
    assert!(report.of(Severity::Warning).any(|d| {
        d.message
            .contains("`verified[0].at` is not an ISO-8601 datetime with an explicit offset")
    }));
    assert!(okf_core::DateTime::parse("2026-06-26").is_some());
    assert!(!okf_validator::validate::is_iso8601_datetime("2026-06-26"));
    assert!(okf_validator::validate::is_iso8601_datetime(
        "2026-06-26T00:00:00Z"
    ));
}

#[test]
fn malformed_and_unreadable_reserved_files_are_conformance_errors() {
    let tmp = TempDir::new();
    tmp.write("a.md", "---\ntype: Note\n---\nBody.\n");
    tmp.write("index.md", "---\nokf_version: 0.2\n");
    tmp.write("nested/index.md", "---\ntype: Not allowed\n---\n");
    tmp.write(
        "log.md",
        "# Log\n\n## someday\n* **Update**: broken date.\n",
    );

    let bundle = Bundle::load(tmp.path()).unwrap();
    let report = validate_bundle(&bundle);
    assert!(!report.is_conformant());
    assert!(
        report
            .of(Severity::Error)
            .any(|d| { d.message.contains("unparseable reserved index.md") })
    );
    assert!(
        report
            .of(Severity::Error)
            .any(|d| { d.message.contains("should not contain frontmatter") })
    );
    assert!(
        report
            .of(Severity::Error)
            .any(|d| { d.message.contains("log date heading is not ISO-8601") })
    );

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
fn log_structure_requires_nonempty_newest_first_date_groups() {
    for (contents, expected) in [
        (
            "# My Update History\n\nThis is not a log.\n",
            "no date groups",
        ),
        ("# My Update History\n\n## 2026-06-26\n", "has no entries"),
        (
            "# My Update History\n\n## 2026-06-26\n* ordinary prose entry\n\n## 2026-06-27\n* another ordinary entry\n",
            "not newest first",
        ),
        (
            "# My Update History\n\n## 2026-06-27\n* ordinary prose entry\nUnexpected prose.\n",
            "non-log content",
        ),
    ] {
        let tmp = TempDir::new();
        tmp.write("a.md", "---\ntype: Note\n---\nBody.\n");
        tmp.write("log.md", contents);
        let report = validate_bundle(&Bundle::load(tmp.path()).unwrap());

        assert!(!report.is_conformant(), "accepted log: {contents:?}");
        assert!(
            report
                .of(Severity::Error)
                .any(|diagnostic| { diagnostic.message.contains(expected) }),
            "{expected:?}: {:#?}",
            report.diagnostics
        );
    }
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
fn log_with_frontmatter_loads_and_conforms() {
    let tmp = TempDir::new();
    tmp.write("a.md", "---\ntype: Note\n---\nBody.\n");
    tmp.write(
        "log.md",
        "---\ntype: Log\ntitle: Bundle history\n---\n\n\
         # Bundle history\n\n\
         ## 2026-07-01\n\
         - **Verified** the full bundle.\n",
    );
    let report = validate_bundle(&Bundle::load(tmp.path()).unwrap());

    assert!(report.is_conformant(), "{:#?}", report.diagnostics);
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
    okf_core::index::regenerate_indexes(tmp.path()).unwrap();

    let root = tmp.read("index.md");
    assert!(
        root.starts_with("---\nokf_version: \"0.2\"\n---\n"),
        "{root}"
    );
    assert!(root.contains("# Subdirectories"), "{root}");

    let bundle = Bundle::load(tmp.path()).unwrap();
    assert_eq!(bundle.okf_version(), Some("0.2"));

    // Attested Computations get their own index section, which is how
    // discovery-by-type works from an index.
    let computations = tmp.read("computations/index.md");
    assert!(
        computations.starts_with("# Attested Computation"),
        "{computations}"
    );
}

#[test]
fn test_validate_warns_when_verified_predates_generated() {
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
    let warnings: Vec<_> = report
        .of(Severity::Warning)
        .filter(|d| d.message.contains("predates `generated.at`"))
        .collect();
    assert_eq!(warnings.len(), 1, "expected warning for verified predating generated: {warnings:#?}");
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
    assert_eq!(warnings.len(), 1, "expected warning for future timestamp: {warnings:#?}");
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
    assert_eq!(warnings.len(), 1, "expected warning for empty body: {warnings:#?}");
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
    assert!(!warnings.is_empty(), "expected warning for circular derivation: {warnings:#?}");
}

#[test]
fn test_validate_warns_on_missing_attestation_resource_on_disk() {
    let tmp = TempDir::new();
    tmp.write("index.md", "# Test\n\n* [Calc](comp.md)\n");
    tmp.write(
        "comp.md",
        "---\ntype: Attested Computation\ntitle: Calc\ndescription: d\n\
         generated: { by: ref/x, at: 2026-06-20T22:53:05Z }\n\
         runtime: python\n\
         parameters:\n  - { name: x, type: string }\n\
         executor:\n  resource: non_existent_script.py\n  receipt: [res]\n\
         attester:\n  resource: non_existent_verifier.py\n\
         ---\n\n# Calc\n\n# Computation\n\n```python\nx = 1\n```\n",
    );
    let bundle = Bundle::load(tmp.path()).unwrap();
    let report = validate_bundle(&bundle);
    let warnings: Vec<_> = report
        .of(Severity::Warning)
        .filter(|d| d.message.contains("which does not exist on disk"))
        .collect();
    assert_eq!(warnings.len(), 2, "expected 2 warnings for missing resources on disk: {warnings:#?}");
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
fn test_validate_warns_on_link_to_deprecated_concept() {
    let tmp = TempDir::new();
    tmp.write("index.md", "# Test\n\n* [New](new.md)\n* [Old](old.md)\n");
    tmp.write(
        "old.md",
        "---\ntype: Metric\ntitle: Old\ndescription: d\nstatus: deprecated\n\
         generated: { by: ref/x, at: 2026-06-20T22:53:05Z }\n\
         ---\n\n# Definition\n\nProse.\n",
    );
    tmp.write(
        "new.md",
        "---\ntype: Metric\ntitle: New\ndescription: d\n\
         generated: { by: ref/x, at: 2026-06-20T22:53:05Z }\n\
         ---\n\n# Definition\n\nSee [the old one](old.md).\n",
    );
    let bundle = Bundle::load(tmp.path()).unwrap();
    let report = validate_bundle(&bundle);
    let warnings: Vec<_> = report
        .of(Severity::Warning)
        .filter(|d| d.message.contains("links to deprecated concept `old`"))
        .collect();
    assert_eq!(warnings.len(), 1, "expected warning for linking to deprecated concept: {warnings:#?}");
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
        .filter(|d| d.message.contains("`title` \"Same title\" is shared with another concept"))
        .collect();
    assert_eq!(warnings.len(), 2, "expected 2 warnings for duplicate titles: {warnings:#?}");
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
    assert_eq!(warnings.len(), 1, "expected warning for duplicate log date: {warnings:#?}");
    assert!(warnings[0].fixable, "duplicate log date warning should be fixable");
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
        .filter(|d| d.message.contains("index.md is out of sync with its directory"))
        .collect();
    assert_eq!(warnings.len(), 1, "expected warning for stale index: {warnings:#?}");
    assert!(warnings[0].fixable, "stale index warning should be fixable");
}
