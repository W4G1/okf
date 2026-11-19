use okf_core::fix::{
    FixOptions, RemediationKind, remediate_bundle, remediate_document, remediate_log,
};
use okf_core::{Bundle, Document, TrustTier};
use std::fs;
use std::path::PathBuf;

/// Helper to create a temporary directory for tests.
struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new() -> Self {
        static CNT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let id = CNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("okf-fix-test-{}-{}", std::process::id(), id));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        Self { path }
    }

    fn path(&self) -> &std::path::Path {
        &self.path
    }

    fn write(&self, rel: &str, content: &str) -> PathBuf {
        let full = self.path.join(rel);
        if let Some(parent) = full.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(&full, content).unwrap();
        full
    }

    fn read(&self, rel: &str) -> String {
        fs::read_to_string(self.path.join(rel)).unwrap()
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

#[test]
fn fixes_missing_title_and_derives_from_stem() {
    let input = "---\ntype: Concept\n---\n# Some Content\n";
    let doc = Document::parse(input).unwrap();
    let opts = FixOptions {
        add_missing_title: true,
        ..Default::default()
    };

    let (remediated, fixes) = remediate_document(&doc, Some("gross_margin"), &opts);
    assert_eq!(
        remediated.frontmatter.title().as_deref(),
        Some("Gross Margin")
    );
    assert!(
        fixes
            .iter()
            .any(|f| matches!(&f.kind, RemediationKind::AddedTitle(t) if t == "Gross Margin"))
    );
}

#[test]
fn fixes_missing_generated_block() {
    let input = "---\ntype: Metric\ntitle: MRR\n---\n# MRR\nMonthly recurring revenue.\n";
    let doc = Document::parse(input).unwrap();
    let opts = FixOptions {
        author: Some("human:charlie".to_string()),
        ..Default::default()
    };

    let (remediated, fixes) = remediate_document(&doc, Some("mrr"), &opts);
    let generated = remediated.frontmatter.generated().unwrap();
    assert_eq!(generated.by.unwrap().as_str(), "human:charlie");
    assert!(generated.at.is_some());
    assert!(
        fixes
            .iter()
            .any(|f| matches!(f.kind, RemediationKind::AddedGenerated))
    );
}

#[test]
fn migrates_legacy_timestamp_to_generated() {
    let input = "---\n\
                 type: Concept\n\
                 title: Architecture\n\
                 timestamp: 2026-03-15T12:00:00Z\n\
                 ---\n\n\
                 # Architecture\n\
                 System design overview.\n";
    let doc = Document::parse(input).unwrap();
    let opts = FixOptions {
        author: Some("process:migrator".to_string()),
        ..Default::default()
    };

    let (remediated, fixes) = remediate_document(&doc, Some("arch"), &opts);
    assert!(remediated.frontmatter.get("timestamp").is_none());
    let generated = remediated.frontmatter.generated().unwrap();
    assert_eq!(generated.by.unwrap().as_str(), "process:migrator");
    assert_eq!(generated.at.unwrap().raw, "2026-03-15T12:00:00Z");
    assert!(
        fixes
            .iter()
            .any(|f| matches!(f.kind, RemediationKind::MigratedTimestamp))
    );
}

#[test]
fn removes_redundant_legacy_timestamp_when_generated_present() {
    let input = "---\n\
                 type: Concept\n\
                 title: Architecture\n\
                 timestamp: 2026-03-15T12:00:00Z\n\
                 generated:\n\
                 \x20 by: human:alice\n\
                 \x20 at: 2026-04-01T10:00:00Z\n\
                 ---\n\n\
                 # Architecture\n\
                 System design overview.\n";
    let doc = Document::parse(input).unwrap();
    let opts = FixOptions::default();

    let (remediated, fixes) = remediate_document(&doc, Some("arch"), &opts);
    assert!(remediated.frontmatter.get("timestamp").is_none());
    let generated = remediated.frontmatter.generated().unwrap();
    assert_eq!(generated.by.unwrap().as_str(), "human:alice");
    assert_eq!(generated.at.unwrap().raw, "2026-04-01T10:00:00Z");
    assert!(
        fixes
            .iter()
            .any(|f| matches!(f.kind, RemediationKind::MigratedTimestamp))
    );
}

#[test]
fn migrates_legacy_citations_section_and_rewrites_body_references() {
    let input = "---\n\
                 type: Concept\n\
                 title: Standards\n\
                 generated:\n\
                   by: human:alice\n\
                   at: 2026-01-01T00:00:00Z\n\
                 ---\n\n\
                 # Standards\n\n\
                 According to [1] and corroborated by [2], the metric follows industry standard.\n\n\
                 # Citations\n\
                 [1] [ISO Standard](https://iso.org/std/123)\n\
                 [2] https://w3.org/spec\n";
    let doc = Document::parse(input).unwrap();
    let opts = FixOptions::default();

    let (remediated, fixes) = remediate_document(&doc, Some("standards"), &opts);
    let sources = remediated.frontmatter.sources();
    assert_eq!(sources.len(), 2);
    assert_eq!(sources[0].id.as_deref(), Some("1"));
    assert_eq!(
        sources[0].resource.as_deref(),
        Some("https://iso.org/std/123")
    );
    assert_eq!(sources[0].title.as_deref(), Some("ISO Standard"));

    assert_eq!(sources[1].id.as_deref(), Some("2"));
    assert_eq!(sources[1].resource.as_deref(), Some("https://w3.org/spec"));

    assert!(!remediated.body.contains("# Citations"));
    assert!(
        remediated
            .body
            .contains("According to [^1] and corroborated by [^2]")
    );
    assert!(
        fixes
            .iter()
            .any(|f| matches!(f.kind, RemediationKind::MigratedCitations))
    );
}

#[test]
fn adds_missing_top_level_heading() {
    let input = "---\n\
                 type: Concept\n\
                 title: Getting Started\n\
                 generated:\n\
                   by: human:alice\n\
                   at: 2026-01-01T00:00:00Z\n\
                 ---\n\n\
                 Welcome to the bundle.\n";
    let doc = Document::parse(input).unwrap();
    let opts = FixOptions::default();

    let (remediated, fixes) = remediate_document(&doc, Some("intro"), &opts);
    assert!(
        remediated
            .body
            .starts_with("# Getting Started\n\nWelcome to the bundle.")
    );
    assert!(
        fixes.iter().any(
            |f| matches!(&f.kind, RemediationKind::AddedTopHeading(t) if t == "Getting Started")
        )
    );
}

#[test]
fn tags_unlabeled_computation_code_block() {
    let input = "---\n\
                 type: Attested Computation\n\
                 title: Calc Revenue\n\
                 runtime: python\n\
                 generated:\n\
                   by: human:alice\n\
                   at: 2026-01-01T00:00:00Z\n\
                 ---\n\n\
                 # Calc Revenue\n\n\
                 # Computation\n\n\
                 ```\n\
                 def compute(a, b):\n\
                     return a + b\n\
                 ```\n";
    let doc = Document::parse(input).unwrap();
    let opts = FixOptions::default();

    let (remediated, fixes) = remediate_document(&doc, Some("calc"), &opts);
    assert!(remediated.body.contains("```python\ndef compute"));
    assert!(
        fixes.iter().any(
            |f| matches!(&f.kind, RemediationKind::AddedComputationLanguage(l) if l == "python")
        )
    );
}

#[test]
fn normalizes_frontmatter_key_order() {
    let input = "---\n\
                 sources:\n\
                 \x20 - { id: s1, resource: https://example.com }\n\
                 title: Messy\n\
                 status: stable\n\
                 type: Concept\n\
                 generated:\n\
                 \x20 by: human:alice\n\
                 \x20 at: 2026-01-01T00:00:00Z\n\
                 ---\n\n\
                 # Messy\n";
    let doc = Document::parse(input).unwrap();
    let opts = FixOptions::default();

    let (remediated, fixes) = remediate_document(&doc, Some("messy"), &opts);
    let keys: Vec<&str> = remediated.frontmatter.as_mapping().keys().collect();
    assert_eq!(
        keys,
        vec!["type", "title", "status", "generated", "sources"]
    );
    assert!(
        fixes
            .iter()
            .any(|f| matches!(f.kind, RemediationKind::KeyOrder))
    );
}

#[test]
fn cleans_trailing_whitespace_and_excess_blank_lines() {
    // Case 1: "sometext    \n"
    let doc1 = Document::new(okf_core::Frontmatter::new(), "# Title\n\nsometext    \n");
    let (fixed1, fixes1) = remediate_document(&doc1, None, &FixOptions::default());
    assert_eq!(fixed1.body, "# Title\n\nsometext\n");
    assert!(
        fixes1
            .iter()
            .any(|f| matches!(f.kind, RemediationKind::CleanedWhitespace))
    );

    // Case 2: "sometext    \n    " -> becomes "sometext\n"
    let doc2 = Document::new(
        okf_core::Frontmatter::new(),
        "# Title\n\nsometext    \n    ",
    );
    let (fixed2, fixes2) = remediate_document(&doc2, None, &FixOptions::default());
    assert_eq!(fixed2.body, "# Title\n\nsometext\n");
    assert!(
        fixes2
            .iter()
            .any(|f| matches!(f.kind, RemediationKind::CleanedWhitespace))
    );

    // Case 3: "sometext     \n   \n   \n     " -> becomes "sometext\n"
    let doc3 = Document::new(
        okf_core::Frontmatter::new(),
        "# Title\n\nsometext     \n   \n   \n     ",
    );
    let (fixed3, fixes3) = remediate_document(&doc3, None, &FixOptions::default());
    assert_eq!(fixed3.body, "# Title\n\nsometext\n");
    assert!(
        fixes3
            .iter()
            .any(|f| matches!(f.kind, RemediationKind::CleanedWhitespace))
    );

    // Case 4: Trailing tabs "sometext\t\t\n"
    let doc4 = Document::new(okf_core::Frontmatter::new(), "# Title\n\nsometext\t\t\n");
    let (fixed4, fixes4) = remediate_document(&doc4, None, &FixOptions::default());
    assert_eq!(fixed4.body, "# Title\n\nsometext\n");
    assert!(
        fixes4
            .iter()
            .any(|f| matches!(f.kind, RemediationKind::CleanedWhitespace))
    );

    // Case 5: Excess blank lines between paragraphs (more than 2 blank lines collapses to 2 blank lines)
    let doc5 = Document::new(
        okf_core::Frontmatter::new(),
        "# Title\n\nsometext\n\n\n\n\nmore\n",
    );
    let (fixed5, fixes5) = remediate_document(&doc5, None, &FixOptions::default());
    assert_eq!(fixed5.body, "# Title\n\nsometext\n\n\nmore\n");
    assert!(
        fixes5
            .iter()
            .any(|f| matches!(f.kind, RemediationKind::CleanedWhitespace))
    );

    // Case 6: Trailing blank lines at end of body "sometext\n\n\n\n"
    let doc6 = Document::new(okf_core::Frontmatter::new(), "# Title\n\nsometext\n\n\n\n");
    let (fixed6, fixes6) = remediate_document(&doc6, None, &FixOptions::default());
    assert_eq!(fixed6.body, "# Title\n\nsometext\n");
    assert!(
        fixes6
            .iter()
            .any(|f| matches!(f.kind, RemediationKind::CleanedWhitespace))
    );
}

#[test]
fn remediates_duplicate_log_dates() {
    let log_text = "# Update Log\n\n\
                    ## 2026-05-10\n\
                    * **Creation**: Created overview.\n\n\
                    ## 2026-05-10\n\
                    * **Update**: Added metric link.\n";
    let (remediated, fixes) = remediate_log(log_text, &FixOptions::default());
    assert_eq!(remediated.matches("## 2026-05-10").count(), 1);
    assert!(remediated.contains("* **Creation**: Created overview."));
    assert!(remediated.contains("* **Update**: Added metric link."));
    assert_eq!(fixes.len(), 1);
}

#[test]
fn remediate_bundle_end_to_end() {
    let tmp = TempDir::new();
    tmp.write(
        "index.md",
        "---\nokf_version: \"0.3\"\n---\n\n# OKF Bundle\n",
    );
    tmp.write(
        "log.md",
        "# Update Log\n\n## 2026-01-01\n* Entry 1\n\n## 2026-01-01\n* Entry 2\n",
    );
    tmp.write(
        "metrics/revenue.md",
        "---\n\
         type: Metric\n\
         timestamp: 2026-01-01T00:00:00Z\n\
         ---\n\n\
         Revenue represents income.\n",
    );

    let opts = FixOptions {
        author: Some("human:alice".to_string()),
        ..Default::default()
    };

    let report = remediate_bundle(tmp.path(), &opts).unwrap();
    assert_eq!(report.total_remediations(), 4); // revenue: title, timestamp, top-heading; log: duplicate date
    let (written, _regenerated) = report.apply().unwrap();
    assert!(written >= 2);

    // Verify loading the bundle now produces a clean bundle
    let bundle = Bundle::load(tmp.path()).unwrap();
    let rev = bundle
        .get(&okf_core::ConceptId::parse("metrics/revenue").unwrap())
        .unwrap();
    assert_eq!(rev.display_title(), "Revenue");
    assert_eq!(
        rev.document
            .frontmatter
            .generated()
            .unwrap()
            .by
            .unwrap()
            .as_str(),
        "human:alice"
    );
    assert!(
        rev.document
            .body
            .starts_with("# Revenue\n\nRevenue represents income.")
    );
}

#[test]
fn remediates_unquoted_okf_version_in_root_index() {
    let tmp = TempDir::new();
    tmp.write("index.md", "---\nokf_version: 0.2\n---\n\n# OKF Bundle\n");
    let opts = FixOptions::default();
    let report = remediate_bundle(tmp.path(), &opts).unwrap();
    assert_eq!(report.total_remediations(), 1);
    let (written, _) = report.apply().unwrap();
    assert_eq!(written, 1);

    let content = fs::read_to_string(tmp.path().join("index.md")).unwrap();
    assert!(
        content.starts_with("---\nokf_version: \"0.2\"\n---\n"),
        "index.md should have quoted okf_version: {content}"
    );
}

#[test]
fn a_fix_rewrites_only_what_it_fixes() {
    let tmp = TempDir::new();
    tmp.write("index.md", "---\nokf_version: \"0.3\"\n---\n\n# Bundle\n");
    // Nothing to fix here, but nothing canonical either: flow mappings,
    // single quotes, a comment, an extension key.
    let clean = "---\n\
        type: Metric\n\
        title: Revenue\n\
        description: Recognized revenue.\n\
        generated: { by: reference_agent/v1, at: '2026-06-20T14:00:00Z' }\n\
        stale_after: \"2026-12-31\"   # reviewed yearly\n\
        x-acme-owner: finance\n\
        ---\n\n# Revenue\n\nBody.\n";
    tmp.write("clean.md", clean);
    // One thing to fix: the offset-less `verified.at`.
    tmp.write(
        "dated.md",
        "---\n\
         type: Metric\n\
         title: Margin\n\
         description: Gross margin.\n\
         generated: { by: reference_agent/v1, at: '2026-06-20T14:00:00Z' }\n\
         verified: { by: \"human:alice\", at: \"2026-06-25 09:00:00\" }\n\
         tags: [finance, margin]   # two tags\n\
         ---\n\n# Margin\n\nBody.\n",
    );
    // Not concepts: a README, and a skill file with frontmatter but no type.
    let readme = "# About\r\n\r\nWindows line endings and all.\r\n";
    let skill = "---\nname: run-on-bq\ndescription: Runs things.\n---\nSteps.\n";
    tmp.write("README.md", readme);
    tmp.write("skills/SKILL.md", skill);

    let report = remediate_bundle(tmp.path(), &FixOptions::validation_only(None)).unwrap();
    report.apply().unwrap();

    assert_eq!(tmp.read("clean.md"), clean);
    assert_eq!(tmp.read("README.md"), readme);
    assert_eq!(tmp.read("skills/SKILL.md"), skill);

    let dated = tmp.read("dated.md");
    assert!(dated.contains("at: \"2026-06-25T09:00:00Z\""), "{dated}");
    // The entries around the fixed one are exactly as written.
    assert!(dated.contains("generated: { by: reference_agent/v1, at: '2026-06-20T14:00:00Z' }\n"));
    assert!(dated.contains("tags: [finance, margin]   # two tags\n"));

    let changed: Vec<_> = report.changed_files().collect();
    assert_eq!(changed.len(), 1);
    // Filling in an offset is a guess, and the report says so.
    assert!(
        changed[0].remediations[0]
            .description
            .contains("UTC assumed"),
        "{:?}",
        changed[0].remediations
    );
}

#[test]
fn valid_timestamps_are_left_alone_and_compared_ones_become_date_times() {
    let doc = Document::parse(
        "---\n\
         type: Metric\n\
         title: T\n\
         generated: { by: tool/v1, at: 2026-06-30 }\n\
         verified:\n\
         \x20 - { by: \"human:a\", at: \"2026-07-01\" }\n\
         \x20 - { by: \"human:b\", at: \"2026-07-02T10:00:00+02:00\" }\n\
         stale_after: 2026-12-31\n\
         usage_window: { from: 2026-06-01, to: \"2026-06-30T00:00:00Z\" }\n\
         sources:\n\
         \x20 - { resource: https://example.com, last_modified: 2026-05-15, usage_count: 1 }\n\
         ---\n\nBody.\n",
    )
    .unwrap();
    let (fixed, remediations) =
        remediate_document(&doc, Some("t"), &FixOptions::validation_only(None));
    let fm = &fixed.frontmatter;

    // `generated.at` and `verified[].at` are compared with each other, so a
    // date there becomes the equivalent date-time.
    assert_eq!(
        fm.generated().unwrap().at.unwrap().raw,
        "2026-06-30T00:00:00Z"
    );
    let verified = fm.verified();
    assert_eq!(verified[0].at.as_ref().unwrap().raw, "2026-07-01T00:00:00Z");
    assert_eq!(
        verified[1].at.as_ref().unwrap().raw,
        "2026-07-02T10:00:00+02:00"
    );
    // A date-only value is valid everywhere else and is not rewritten.
    assert_eq!(fm.stale_after().unwrap().raw, "2026-12-31");
    assert_eq!(fm.usage_window().unwrap().from.unwrap().raw, "2026-06-01");
    assert_eq!(
        fm.sources()[0].last_modified.as_ref().unwrap().raw,
        "2026-05-15"
    );
    assert_eq!(remediations.len(), 2, "{remediations:?}");
}

#[test]
fn migrations_from_v0_2_are_reported_one_by_one() {
    let tmp = TempDir::new();
    tmp.write("index.md", "---\ntitle: Legacy bundle\n---\n\n# Bundle\n");
    tmp.write(
        "log.md",
        "---\nx-note: old\n---\n\n# Log\n\n## 2026-06-01\n* **Creation**: Made.\n",
    );
    tmp.write("references/run.md", "# Run\n");
    tmp.write(
        "computations/c.md",
        "---\ntype: Attested Computation\ntitle: C\ndescription: d\ntags: finance\nruntime: python\n\
         generated: { by: tool/v1, at: \"2026-06-20T14:00:00Z\" }\n\
         executor: { resource: references/run.md, receipt: [id] }\n\
         ---\n\n# C\n\n# Computation\n\n\tx = 1\n\ty = 2\n",
    );

    let report = remediate_bundle(tmp.path(), &FixOptions::validation_only(None)).unwrap();
    let kinds: Vec<&RemediationKind> = report
        .files
        .iter()
        .flat_map(|file| file.remediations.iter().map(|r| &r.kind))
        .collect();
    assert!(
        kinds.contains(&&RemediationKind::ListifiedTags),
        "{kinds:?}"
    );
    assert!(
        kinds.contains(&&RemediationKind::FencedComputation),
        "{kinds:?}"
    );
    assert!(
        kinds.contains(&&RemediationKind::StrippedLogFrontmatter),
        "{kinds:?}"
    );
    assert!(
        kinds.contains(&&RemediationKind::DeclaredOkfVersion),
        "{kinds:?}"
    );
    assert!(
        kinds.contains(&&RemediationKind::RootedPath(
            "executor.resource".to_string()
        )),
        "{kinds:?}"
    );
    report.apply().unwrap();

    let concept = tmp.read("computations/c.md");
    assert!(concept.contains("tags:\n  - finance\n"), "{concept}");
    assert!(
        concept.contains("resource: /references/run.md"),
        "{concept}"
    );
    assert!(
        concept.contains("# Computation\n\n```python\nx = 1\ny = 2\n```\n"),
        "{concept}"
    );
    // The version is added to the existing block; the title stays put.
    assert_eq!(
        tmp.read("index.md"),
        "---\nokf_version: \"0.3\"\ntitle: Legacy bundle\n---\n\n# Bundle\n"
    );
    assert_eq!(
        tmp.read("log.md"),
        "# Log\n\n## 2026-06-01\n* **Creation**: Made.\n"
    );

    // The bundle now reads the same with or without the fallbacks.
    let bundle = Bundle::load(tmp.path()).unwrap();
    assert!(!bundle.legacy_fallbacks_apply());
    assert!(bundle.legacy_path_fields().is_empty());
    assert!(
        remediate_bundle(tmp.path(), &FixOptions::validation_only(None))
            .unwrap()
            .is_empty()
    );
}

#[test]
fn only_an_untagged_opening_fence_gets_the_runtime() {
    // The fence is already tagged, with the language rather than the runtime.
    // There is nothing to add, and the closing fence is never a place for a
    // tag.
    let tagged = "---\n\
                  type: Attested Computation\n\
                  title: Revenue\n\
                  runtime: bigquery\n\
                  generated:\n\
                  \x20 by: human:alice\n\
                  \x20 at: 2026-01-01T00:00:00Z\n\
                  ---\n\n\
                  # Revenue\n\n\
                  # Computation\n\n\
                  ```sql\n\
                  SELECT 1\n\
                  ```\n";
    let doc = Document::parse(tagged).unwrap();
    let (remediated, fixes) = remediate_document(&doc, Some("revenue"), &FixOptions::default());
    assert_eq!(remediated.body, doc.body);
    assert_eq!(fixes, Vec::new());

    // Untagged: the opening fence gets the runtime, the closing one stays
    // bare, and a later block in the section is not the computation.
    let untagged = tagged.replace(
        "```sql\nSELECT 1\n```\n",
        "```\nSELECT 1\n```\n\nExample output:\n\n```\n1\n```\n",
    );
    let doc = Document::parse(&untagged).unwrap();
    let (remediated, fixes) = remediate_document(&doc, Some("revenue"), &FixOptions::default());
    assert!(
        remediated
            .body
            .ends_with("```bigquery\nSELECT 1\n```\n\nExample output:\n\n```\n1\n```"),
        "{}",
        remediated.body
    );
    assert_eq!(fixes.len(), 1);
}

#[test]
fn the_fence_fix_does_not_reach_into_another_code_block() {
    // The `# Computation` line is inside a fenced example, so it opens no
    // section, and the indented lines below it are part of that example.
    let input = "---\n\
                 type: Note\n\
                 title: How to write a computation\n\
                 generated:\n\
                 \x20 by: human:alice\n\
                 \x20 at: 2026-01-01T00:00:00Z\n\
                 ---\n\n\
                 # How to write a computation\n\n\
                 ```markdown\n\
                 # Computation\n\n\
                 \x20   SELECT 1\n\
                 ```\n";
    let doc = Document::parse(input).unwrap();
    let options = FixOptions {
        fence_computation: true,
        ..Default::default()
    };
    let (remediated, fixes) = remediate_document(&doc, Some("how-to"), &options);
    assert_eq!(remediated.body, doc.body);
    assert_eq!(fixes, Vec::new());
}

#[test]
fn a_generated_block_added_to_a_reviewed_concept_carries_no_time() {
    // Trust comes from verifications at or after `generated.at`. Stamping
    // "now" on a concept that was reviewed earlier would say it changed after
    // its review, and take the review away. The fix does not know when the
    // concept was written, so it does not claim to.
    let input = "---\n\
                 type: Metric\n\
                 title: MRR\n\
                 verified:\n\
                 \x20 - by: human:jane\n\
                 \x20   at: 2026-01-10T09:00:00Z\n\
                 ---\n# MRR\nMonthly recurring revenue.\n";
    let doc = Document::parse(input).unwrap();
    assert_eq!(doc.frontmatter.trust_tier(), TrustTier::HumanReviewed);
    let options = FixOptions {
        author: Some("process:migrator".to_string()),
        ..Default::default()
    };

    let (remediated, fixes) = remediate_document(&doc, Some("mrr"), &options);
    let generated = remediated.frontmatter.generated().unwrap();
    assert_eq!(generated.by.unwrap().as_str(), "process:migrator");
    assert!(generated.at.is_none());
    assert_eq!(
        remediated.frontmatter.trust_tier(),
        TrustTier::HumanReviewed
    );
    assert!(
        fixes
            .iter()
            .any(|f| matches!(f.kind, RemediationKind::AddedGenerated))
    );
}

#[test]
fn a_legacy_timestamp_fills_a_generated_block_that_has_no_time() {
    // `timestamp` is the only record of when the concept was written. It
    // moves into `generated.at` rather than being dropped.
    let input = "---\n\
                 type: Concept\n\
                 title: Architecture\n\
                 timestamp: 2026-03-15T12:00:00Z\n\
                 generated:\n\
                 \x20 by: human:alice\n\
                 ---\n\n\
                 # Architecture\n\
                 System design overview.\n";
    let doc = Document::parse(input).unwrap();
    let (remediated, fixes) = remediate_document(&doc, Some("arch"), &FixOptions::default());
    assert!(remediated.frontmatter.get("timestamp").is_none());
    let generated = remediated.frontmatter.generated().unwrap();
    assert_eq!(generated.by.unwrap().as_str(), "human:alice");
    assert_eq!(generated.at.unwrap().raw, "2026-03-15T12:00:00Z");
    assert!(
        fixes
            .iter()
            .any(|f| matches!(f.kind, RemediationKind::MigratedTimestamp))
    );
}

#[test]
fn citation_migration_keeps_text_that_is_not_ascii() {
    let input = "---\n\
                 type: Concept\n\
                 title: Normen\n\
                 generated:\n\
                 \x20 by: human:alice\n\
                 \x20 at: 2026-01-01T00:00:00Z\n\
                 ---\n\n\
                 # Normen\n\n\
                 Volgens [1] geldt 21 % btw — café, naïef, 日本語 [2] en “aanhalingstekens”.\n\n\
                 # Citations\n\
                 [1] [Wet op de omzetbelasting](https://example.org/wet)\n\
                 [2] https://example.org/日本語\n";
    let doc = Document::parse(input).unwrap();
    let (remediated, _) = remediate_document(&doc, Some("normen"), &FixOptions::default());
    assert!(
        remediated.body.contains(
            "Volgens [^1] geldt 21 % btw — café, naïef, 日本語 [^2] en “aanhalingstekens”."
        ),
        "{}",
        remediated.body
    );
}

#[test]
fn consolidating_log_dates_keeps_every_line_as_written() {
    // Entries are moved as text: a wrapped line, a nested bullet, and the
    // file's own bullet style all survive.
    let log_text = "# Update Log\n\n\
                    ## 2026-05-10\n\
                    - **Creation**: Created the overview,\n\
                    \x20 which took a while.\n\
                    \x20 - with a nested point\n\n\
                    ## 2026-05-09\n\
                    - **Update**: Earlier work.\n\n\
                    ## 2026-05-10\n\
                    - **Update**: Added a [metric](/metrics/mrr.md).\n";
    let (remediated, fixes) = remediate_log(log_text, &FixOptions::default());
    assert_eq!(
        remediated,
        "# Update Log\n\n\
         ## 2026-05-10\n\
         - **Creation**: Created the overview,\n\
         \x20 which took a while.\n\
         \x20 - with a nested point\n\
         - **Update**: Added a [metric](/metrics/mrr.md).\n\n\
         ## 2026-05-09\n\
         - **Update**: Earlier work.\n"
    );
    assert_eq!(fixes.len(), 1);

    // A log with nothing to fix comes back byte for byte.
    let (again, fixes) = remediate_log(&remediated, &FixOptions::default());
    assert_eq!(again, remediated);
    assert_eq!(fixes, Vec::new());
}

#[cfg(unix)]
#[test]
fn a_fix_does_not_follow_a_link_out_of_the_bundle() {
    // Both files need a fix (no title, no top-level heading). The one outside
    // the bundle is reachable through a linked file and a linked directory,
    // and is none of this bundle's business.
    let outside = TempDir::new();
    let foreign = "---\ntype: Note\n---\nSomebody else's note.\n";
    outside.write("foreign.md", foreign);
    outside.write("dir/nested.md", foreign);

    let tmp = TempDir::new();
    tmp.write("own.md", "---\ntype: Note\n---\nOur note.\n");
    std::os::unix::fs::symlink(
        outside.path().join("foreign.md"),
        tmp.path().join("linked.md"),
    )
    .unwrap();
    std::os::unix::fs::symlink(outside.path().join("dir"), tmp.path().join("linked-dir")).unwrap();

    let report = remediate_bundle(tmp.path(), &FixOptions::default()).unwrap();
    report.apply().unwrap();

    assert!(tmp.read("own.md").contains("title: Own"));
    assert_eq!(outside.read("foreign.md"), foreign);
    assert_eq!(outside.read("dir/nested.md"), foreign);
}
