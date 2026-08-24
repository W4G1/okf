//! Tests for okf-core scaffolding utilities (`init_bundle` and `create_concept`).

mod common;

use common::TempDir;
use okf_core::bundle::Bundle;
use okf_core::scaffold::{
    BundleInitOptions, ConceptOptions, create_concept, default_author, init_bundle, title_from_name,
};
use okf_core::trust::Status;

#[test]
fn title_from_name_formats_names() {
    assert_eq!(
        title_from_name("monthly_active_users"),
        "Monthly Active Users"
    );
    assert_eq!(title_from_name("user-churn-rate"), "User Churn Rate");
    assert_eq!(title_from_name("revenue.md"), "Revenue");
    assert_eq!(title_from_name(""), "Untitled");
}

#[test]
fn default_author_is_valid() {
    let author = default_author();
    assert!(author.starts_with("human:"));
}

#[test]
fn init_bundle_creates_conformant_bundle() {
    let tmp = TempDir::new();
    let bundle_dir = tmp.path().join("my_bundle");

    let options = BundleInitOptions {
        title: "Test Bundle".to_string(),
        create_sample: true,
        sample_name: "overview".to_string(),
        author: Some("human:alice".to_string()),
        force: false,
    };

    let created = init_bundle(&bundle_dir, &options).unwrap();
    assert_eq!(created.len(), 3);

    // Verify bundle loads and has overview concept
    let bundle = Bundle::load(&bundle_dir).unwrap();
    assert_eq!(bundle.len(), 1);
    assert_eq!(bundle.okf_version(), Some("0.2"));

    let overview = bundle
        .get(&okf_core::ConceptId::parse("overview").unwrap())
        .unwrap();
    assert_eq!(overview.display_title(), "Overview");
    assert_eq!(overview.status(), Status::Draft);
}

#[test]
fn init_bundle_bare_creates_no_concepts() {
    let tmp = TempDir::new();
    let bundle_dir = tmp.path().join("bare_bundle");

    let options = BundleInitOptions {
        title: "Bare Base".to_string(),
        create_sample: false,
        sample_name: "overview".to_string(),
        author: None,
        force: false,
    };

    let created = init_bundle(&bundle_dir, &options).unwrap();
    assert_eq!(created.len(), 2); // index.md and log.md

    let bundle = Bundle::load(&bundle_dir).unwrap();
    assert_eq!(bundle.len(), 0);
    assert_eq!(bundle.okf_version(), Some("0.2"));
}

#[test]
fn create_concept_and_attested_computation() {
    let tmp = TempDir::new();
    let concept_path = tmp.path().join("metrics/revenue.md");

    let options = ConceptOptions {
        type_: "Metric".to_string(),
        title: Some("Revenue".to_string()),
        description: Some("Recognized revenue".to_string()),
        status: Status::Stable,
        author: Some("human:bob".to_string()),
        attested: false,
        tags: vec!["finance".to_string()],
        force: false,
    };

    let created = create_concept(&concept_path, &options).unwrap();
    assert_eq!(created, concept_path);

    let doc = okf_core::Document::parse(&std::fs::read_to_string(&concept_path).unwrap()).unwrap();
    assert_eq!(doc.frontmatter.type_().as_deref(), Some("Metric"));
    assert_eq!(doc.frontmatter.title().as_deref(), Some("Revenue"));
    assert_eq!(doc.frontmatter.tags(), vec!["finance"]);

    // Test attested computation
    let contract_path = tmp.path().join("computations/calc_revenue.md");
    let comp_options = ConceptOptions {
        type_: "Attested Computation".to_string(),
        title: Some("Revenue Calculation".to_string()),
        description: Some("Calculation of revenue".to_string()),
        status: Status::Draft,
        author: Some("human:bob".to_string()),
        attested: true,
        tags: Vec::new(),
        force: false,
    };
    create_concept(&contract_path, &comp_options).unwrap();

    let comp_doc =
        okf_core::Document::parse(&std::fs::read_to_string(&contract_path).unwrap()).unwrap();
    let contract = comp_doc.attested_computation().unwrap();
    assert_eq!(contract.runtime.as_deref(), Some("python"));
    assert_eq!(contract.parameters.len(), 1);
    assert!(contract.executor.is_some());
    assert!(contract.attester.is_some());
}
