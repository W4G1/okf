//! Regression tests for the machine-readable cross-link graph.

mod common;

use common::TempDir;
use std::process::Command;

fn graph_json(bundle: &TempDir) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_okf"))
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
            "okf_version": "0.2",
            "concepts": [
                {
                    "id": "plain",
                    "links": [],
                    "sources": []
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
            "okf_version": "0.2",
            "concepts": [
                {
                    "id": "source",
                    "links": [
                        {
                            "target": "target",
                            "exists": true,
                            "text": "say \"hi\"",
                            "raw": "target.md"
                        },
                        {
                            "target": "missing\"target",
                            "exists": false,
                            "text": "missing",
                            "raw": "missing\"target.md"
                        }
                    ],
                    "sources": []
                },
                {
                    "id": "target",
                    "links": [],
                    "sources": []
                }
            ]
        })
    );
}
