use okf_core::bundle::Bundle;
use okf_validator::lint::lint_bundle;
use okf_validator::syntax::{check_syntax, extract_fenced_code_blocks};
use okf_validator::validate::{Severity, validate_bundle};

mod common;
use common::TempDir;

#[test]
fn test_python_syntax_checking() {
    assert!(check_syntax("python", "def add(a, b):\n    return a + b\n").is_ok());
    assert!(check_syntax("py", "x = {'a': 1, 'b': [2, 3]}\n").is_ok());

    let err = check_syntax("python", "def broken(\n").unwrap_err();
    assert_eq!(err.language, "python");
    assert!(err.line.is_some());
}

#[test]
fn test_javascript_and_typescript_syntax_checking() {
    assert!(check_syntax("javascript", "const fn = (x) => x * 2;\nexport default fn;").is_ok());
    assert!(check_syntax("js", "function calculate(total) { return total * 0.2; }").is_ok());
    assert!(
        check_syntax(
            "typescript",
            "interface User { id: number; name: string; }\nconst u: User = { id: 1, name: 'Alice' };"
        )
        .is_ok()
    );
    assert!(
        check_syntax(
            "ts",
            "type ID = string | number;\nexport const id: ID = 42;"
        )
        .is_ok()
    );

    let js_err = check_syntax("javascript", "const a = ;").unwrap_err();
    assert_eq!(js_err.language, "javascript");

    let ts_err = check_syntax("typescript", "interface { invalid }").unwrap_err();
    assert_eq!(ts_err.language, "typescript");
}

#[test]
fn test_rust_syntax_checking() {
    assert!(check_syntax("rust", "pub fn compute(val: i32) -> i32 {\n    val * 2\n}").is_ok());
    // Snippet statements without fn main
    assert!(check_syntax("rust", "let mut x = vec![1, 2, 3];\nx.push(4);").is_ok());

    let err = check_syntax("rust", "fn invalid( { ").unwrap_err();
    assert_eq!(err.language, "rust");
}

#[test]
fn test_sql_syntax_checking() {
    assert!(
        check_syntax(
            "sql",
            "SELECT id, name, count(*) FROM users WHERE active = 1 GROUP BY id, name;"
        )
        .is_ok()
    );
    assert!(
        check_syntax(
            "sql",
            "CREATE TABLE metrics (id INT PRIMARY KEY, value FLOAT);"
        )
        .is_ok()
    );

    let err = check_syntax("sql", "SELECT * FROM (").unwrap_err();
    assert_eq!(err.language, "sql");
}

#[test]
fn test_json_and_yaml_syntax_checking() {
    assert!(check_syntax("json", "{\"key\": [1, 2, 3], \"active\": true}").is_ok());
    let json_err = check_syntax("json", "{\"key\": [1, 2, 3, ]}").unwrap_err();
    assert_eq!(json_err.language, "json");

    assert!(check_syntax("yaml", "key:\n  nested: value\n  list:\n    - 1\n    - 2\n").is_ok());
    let yaml_err = check_syntax("yaml", "key: [unclosed\n").unwrap_err();
    assert_eq!(yaml_err.language, "yaml");
}

#[test]
fn test_bash_syntax_checking() {
    assert!(
        check_syntax(
            "bash",
            "#!/bin/bash\nif [ \"$1\" = \"test\" ]; then\n    echo ok\nfi\n"
        )
        .is_ok()
    );
    assert!(check_syntax("sh", "for item in a b c; do\n    echo \"$item\"\ndone").is_ok());

    let err = check_syntax("bash", "echo \"unclosed string").unwrap_err();
    assert_eq!(err.language, "bash");
}

#[test]
fn test_extract_fenced_code_blocks() {
    let markdown = r#"
# Heading

Some text before.

```python
x = 1
y = 2
```

Middle text.

~~~typescript
interface Config {
    debug: boolean;
}
~~~

```
no language tag
```
"#;

    let blocks = extract_fenced_code_blocks(markdown);
    assert_eq!(blocks.len(), 3);
    assert_eq!(blocks[0].language.as_deref(), Some("python"));
    assert_eq!(blocks[0].code.trim(), "x = 1\ny = 2");
    assert_eq!(blocks[1].language.as_deref(), Some("typescript"));
    assert_eq!(blocks[2].language, None);
}

#[test]
fn test_validate_warns_on_invalid_code_blocks_in_concepts() {
    let tmp = TempDir::new();
    tmp.write(
        "policy.md",
        r#"---
type: Concept
title: Travel Policy
description: Guidelines
generated:
  by: ref/author
  at: 2026-01-01T00:00:00Z
---

# Travel Policy

Here is an example python script:

```python
def broken_syntax(
```

And valid JSON:

```json
{"status": "ok"}
```
"#,
    );
    tmp.write("index.md", "# Index\n\n* [Travel Policy](policy.md)\n");

    let bundle = Bundle::load(tmp.path()).unwrap();
    let val_report = validate_bundle(&bundle);

    // Syntax warnings do not break bundle conformance
    assert!(val_report.is_conformant());

    let syntax_warnings: Vec<_> = val_report
        .diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Warning && d.message.contains("syntax check failed"))
        .collect();

    assert_eq!(
        syntax_warnings.len(),
        1,
        "Expected syntax warning for invalid python code block: {syntax_warnings:#?}"
    );
    assert!(
        syntax_warnings[0].message.contains("python"),
        "Diagnostic message: {}",
        syntax_warnings[0].message
    );

    // lint_bundle should not contain syntax findings
    let lint_report = lint_bundle(&bundle);
    assert!(
        !lint_report
            .diagnostics
            .iter()
            .any(|d| d.message.contains("syntax check failed")),
        "lint_bundle should only check formatting/style issues"
    );
}

#[test]
fn test_validate_warns_on_invalid_attested_computation_file() {
    let tmp = TempDir::new();
    tmp.write(
        "computations/calc.md",
        r#"---
type: Attested Computation
title: Revenue Calculator
description: Computes revenue
generated:
  by: ref/author
  at: 2026-01-01T00:00:00Z
computation: calc.py
runtime: python
executor:
  name: runner
  resource: executor.sh
  receipt: [run.log]
attester:
  name: checker
  resource: attester.py
---

# Revenue Calculator
"#,
    );
    // Write invalid python file referenced in computation
    tmp.write("computations/calc.py", "def broken_code(\n");
    // Write invalid bash file referenced in executor
    tmp.write("computations/executor.sh", "echo \"unclosed\n");
    // Write valid attester script
    tmp.write(
        "computations/attester.py",
        "def verify():\n    return True\n",
    );
    tmp.write(
        "index.md",
        "# Index\n\n* [Revenue Calculator](computations/calc.md)\n",
    );

    let bundle = Bundle::load(tmp.path()).unwrap();
    let val_report = validate_bundle(&bundle);

    assert!(val_report.is_conformant());

    let syntax_warnings: Vec<_> = val_report
        .diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Warning && d.message.contains("syntax check failed"))
        .collect();

    assert_eq!(
        syntax_warnings.len(),
        2,
        "Expected 2 syntax warnings (computation script & executor script): {syntax_warnings:#?}"
    );
    assert!(
        syntax_warnings
            .iter()
            .any(|d| d.message.contains("calc.py")),
        "Expected finding for calc.py"
    );
    assert!(
        syntax_warnings
            .iter()
            .any(|d| d.message.contains("executor.sh")),
        "Expected finding for executor.sh"
    );
}
