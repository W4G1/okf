use okf_core::fix::{
    FixOptions, RemediationKind, remediate_bundle, remediate_document, remediate_log,
};
use okf_core::{Bundle, Document};
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
        "---\nokf_version: \"0.2\"\n---\n\n# OKF Bundle\n",
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
