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
    assert_eq!(graph_json(&empty), "{\n  \"concepts\": [\n  ]\n}\n");

    let linkless = TempDir::new();
    linkless.write("plain.md", "---\ntype: Metric\n---\n\nNo links.\n");
    assert_eq!(
        graph_json(&linkless),
        r#"{
  "concepts": [
    {
      "id": "plain",
      "links": [],
      "sources": []
    }
  ]
}
"#
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

    assert_eq!(
        graph_json(&bundle),
        r#"{
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
}
"#
    );
}
