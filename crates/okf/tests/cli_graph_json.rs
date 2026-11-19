//! Regression tests for the machine-readable cross-link graph.

mod common;

use common::TempDir;
use std::process::Command;

fn okf() -> Command {
    Command::new(env!("CARGO_BIN_EXE_okf"))
}

fn graph_json(bundle: &TempDir) -> String {
    let output = okf()
        .args(["graph", bundle.path().to_str().unwrap(), "--format", "json"])
        .output()
        .unwrap();
    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    String::from_utf8(output.stdout).unwrap()
}

#[test]
fn graph_json_is_valid_for_empty_and_linkless_bundles() {
    let empty = TempDir::new();
    let val: serde_json::Value = serde_json::from_str(&graph_json(&empty)).unwrap();
    assert_eq!(val["concepts"], serde_json::json!([]));

    let linkless = TempDir::new();
    linkless.write("plain.md", "---\ntype: Metric\n---\n\nNo links.\n");
    let val: serde_json::Value = serde_json::from_str(&graph_json(&linkless)).unwrap();
    assert_eq!(
        val,
        serde_json::json!({
            "okf_version": "0.3",
            "concepts": [
                {
                    "id": "plain",
                    "links": [],
                    "sources": [],
                    "path_fields": []
                }
            ]
        })
    );
}

#[test]
fn graph_json_separates_and_escapes_every_link_field() {
    let bundle = TempDir::new();
    bundle.write(
        "source.md",
        "---\ntype: Metric\n---\n\nSee [say \"hi\"](target.md) and [missing](missing\"target.md).\n",
    );
    bundle.write("target.md", "---\ntype: Metric\n---\n\nTarget.\n");

    let val: serde_json::Value = serde_json::from_str(&graph_json(&bundle)).unwrap();
    assert_eq!(
        val,
        serde_json::json!({
            "okf_version": "0.3",
            "concepts": [
                {
                    "id": "source",
                    "links": [
                        {
                            "target": "target",
                            "exists": true,
                            "resolution": "concept",
                            "text": "say \"hi\"",
                            "raw": "target.md"
                        },
                        {
                            "target": "missing\"target",
                            "exists": false,
                            "resolution": "missing",
                            "text": "missing",
                            "raw": "missing\"target.md"
                        }
                    ],
                    "sources": [],
                    "path_fields": []
                },
                {
                    "id": "target",
                    "links": [],
                    "sources": [],
                    "path_fields": []
                }
            ]
        })
    );
}

#[test]
fn graph_fields_adds_path_field_edges() {
    let bundle = TempDir::new();
    bundle.write("index.md", "---\nokf_version: \"0.3\"\n---\n\n# Metric\n");
    bundle.write("metrics/margin.md", "---\ntype: Metric\n---\n\nCurrent.\n");
    bundle.write(
        "metrics/margin-legacy.md",
        "---\ntype: Metric\nstatus: deprecated\nsuperseded_by: /metrics/margin.md\n\
         sources:\n  - { resource: ../policies/margin.md }\n---\n\nRetired. See the [chart](../img/chart.png).\n",
    );
    bundle.write("policies/margin.md", "---\ntype: Policy\n---\n\nPolicy.\n");
    bundle.write("img/chart.png", "png");
    let root = bundle.path().to_str().unwrap();
    let run = |extra: &[&str]| -> String {
        let mut args = vec!["graph", root];
        args.extend_from_slice(extra);
        String::from_utf8(okf().args(&args).output().unwrap().stdout).unwrap()
    };

    // Links only by default, and a link to an asset is not a concept edge.
    assert_eq!(run(&[]), "");
    // `--sources` adds the derivation edges, `--fields` every path field.
    assert_eq!(
        run(&["--sources"]),
        "metrics/margin-legacy\n  ~> policies/margin (source)\n"
    );
    assert_eq!(
        run(&["--fields"]),
        "metrics/margin-legacy\n  ~> metrics/margin (superseded_by)\n  ~> policies/margin (source)\n"
    );
    assert!(run(&["--fields", "--format", "mermaid"]).contains("-.->|superseded_by|"));

    let val: serde_json::Value = serde_json::from_str(&run(&["--fields", "--json"])).unwrap();
    let legacy = &val["concepts"][0];
    assert_eq!(legacy["id"], "metrics/margin-legacy");
    assert_eq!(legacy["links"][0]["resolution"], "file");
    assert_eq!(legacy["sources"], serde_json::json!(["policies/margin"]));
    assert_eq!(
        legacy["path_fields"],
        serde_json::json!([
            { "field": "superseded_by", "target": "metrics/margin", "raw": "/metrics/margin.md" },
            { "field": "sources[0].resource", "target": "policies/margin", "raw": "../policies/margin.md" }
        ])
    );
}
