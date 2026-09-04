use okf_core::bundle::Bundle;
use okf_validator::syntax::{Language, check_syntax, extract_fenced_code_blocks};
use okf_validator::validate::{Severity, validate_bundle};

mod common;
use common::TempDir;

#[cfg(feature = "python")]
#[test]
fn test_python_syntax_checking() {
    assert!(check_syntax("python", "def add(a, b):\n    return a + b\n").is_ok());
    assert!(check_syntax("py", "x = {'a': 1, 'b': [2, 3]}\n").is_ok());

    let err = check_syntax("python", "def broken(\n").unwrap_err();
    assert_eq!(err.language, "python");
    assert!(err.line.is_some());
}

#[cfg(feature = "javascript")]
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

#[cfg(feature = "rust")]
#[test]
fn test_rust_syntax_checking() {
    assert!(check_syntax("rust", "pub fn compute(val: i32) -> i32 {\n    val * 2\n}").is_ok());
    // Snippet statements without fn main
    assert!(check_syntax("rust", "let mut x = vec![1, 2, 3];\nx.push(4);").is_ok());

    let err = check_syntax("rust", "fn invalid( { ").unwrap_err();
    assert_eq!(err.language, "rust");
}

#[cfg(feature = "sql")]
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
fn test_is_supported_tracks_parser_features() {
    assert_eq!(Language::Python.is_supported(), cfg!(feature = "python"));
    assert_eq!(
        Language::JavaScript.is_supported(),
        cfg!(feature = "javascript")
    );
    assert_eq!(
        Language::TypeScript.is_supported(),
        cfg!(feature = "javascript")
    );
    assert_eq!(Language::Rust.is_supported(), cfg!(feature = "rust"));
    assert_eq!(Language::Sql.is_supported(), cfg!(feature = "sql"));

    // Built-in checkers need no feature.
    assert!(Language::Json.is_supported());
    assert!(Language::Yaml.is_supported());
    assert!(Language::Bash.is_supported());
    assert!(!Language::Unknown.is_supported());
}

#[test]
fn test_unsupported_languages_are_accepted_unchecked() {
    // An unknown tag is never an error.
    assert!(check_syntax("cobol", "PROCEDURE DIVISION.").is_ok());
    assert!(check_syntax("", "anything at all").is_ok());

    // A language whose parser is compiled out behaves like an unknown tag:
    // broken source is accepted. With the parser present it is rejected.
    for (lang, broken) in [
        (Language::Python, "def broken(\n"),
        (Language::JavaScript, "const a = ;"),
        (Language::TypeScript, "interface { invalid }"),
        (Language::Rust, "fn invalid( { "),
        (Language::Sql, "SELECT * FROM ("),
    ] {
        let result = check_syntax(lang.as_str(), broken);
        assert_eq!(result.is_err(), lang.is_supported(), "{lang}: {result:?}");
    }
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
fn test_validate_does_not_check_illustrative_code_blocks() {
    // Documentation blocks are frequently fragments: the spec's own reference
    // bundles carry bare join conditions and bare formulas under ```sql, and
    // a Concept may show a deliberately elided snippet. None of that is a
    // conformance question, so no fenced block outside an Attested
    // Computation's `# Computation` section is syntax-checked.
    let tmp = TempDir::new();
    tmp.write(
        "joins/posts__votes.md",
        r#"---
type: Reference
title: posts ⟷ votes
description: Join path
generated:
  by: ref/author
  at: 2026-01-01T00:00:00Z
---

# posts ⟷ votes

Join relationship between the votes table and the posts table.

```sql
ON votes.post_id = posts.id
```

## Formula

```sql
SAFE_DIVIDE(
  COUNT(AcceptedAnswerId),
  COUNT(Id)
)
```

An elided example:

```python
def broken_syntax(
```

And valid JSON:

```json
{"status": "ok"}
```
"#,
    );
    tmp.write(
        "index.md",
        "# Index\n\n* [posts ⟷ votes](joins/posts__votes.md)\n",
    );

    let bundle = Bundle::load(tmp.path()).unwrap();
    let val_report = validate_bundle(&bundle);
    assert!(val_report.is_conformant());

    let syntax_findings: Vec<_> = val_report
        .diagnostics
        .iter()
        .filter(|d| d.message.contains("syntax check failed"))
        .collect();
    assert!(
        syntax_findings.is_empty(),
        "illustrative code blocks must not be syntax-checked: {syntax_findings:#?}"
    );

    // lint_bundle should not contain syntax findings either
    let lint_report = okf_validator::lint::lint_bundle(&bundle);
    assert!(
        !lint_report
            .diagnostics
            .iter()
            .any(|d| d.message.contains("syntax check failed")),
        "lint_bundle should only check formatting/style issues"
    );
}

#[test]
fn test_validate_warns_on_invalid_inline_computation_block() {
    // The `# Computation` block of an Attested Computation is the sanctioned
    // computation an agent executes, so it is checked as a whole statement.
    // The language comes from the fence tag, falling back to `runtime`.
    let tmp = TempDir::new();
    let contract = |title: &str, fence: &str| {
        format!(
            "---\ntype: Attested Computation\ntitle: {title}\ndescription: d\n\
             generated:\n  by: ref/author\n  at: 2026-01-01T00:00:00Z\n\
             runtime: sql\nparameters:\n  - {{ name: year, type: integer }}\n\
             ---\n\n# {title}\n\n# Computation\n\n```{fence}\nSELECT * FROM (\n```\n"
        )
    };
    tmp.write("computations/tagged.md", &contract("Tagged", "sql"));
    tmp.write("computations/untagged.md", &contract("Untagged", ""));
    tmp.write(
        "index.md",
        "# Index\n\n* [Tagged](computations/tagged.md)\n* [Untagged](computations/untagged.md)\n",
    );

    let bundle = Bundle::load(tmp.path()).unwrap();
    let val_report = validate_bundle(&bundle);

    // Syntax warnings do not break bundle conformance
    assert!(val_report.is_conformant());

    let syntax_warnings: Vec<_> = val_report
        .diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Warning && d.message.contains("syntax check failed"))
        .collect();

    // Both bodies are broken SQL; both are reported when the SQL parser is
    // compiled in, and both are accepted unchecked when it is not.
    let expected = if cfg!(feature = "sql") { 2 } else { 0 };
    assert_eq!(
        syntax_warnings.len(),
        expected,
        "Expected `# Computation` syntax warnings to follow the sql feature: {syntax_warnings:#?}"
    );
    for warning in &syntax_warnings {
        assert!(
            warning.message.contains("`# Computation` code block")
                && warning.message.contains("sql"),
            "Diagnostic message: {}",
            warning.message
        );
    }
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

    // The Bash executor script is always checked; the Python computation
    // script only when the `python` parser is compiled in. Without it the
    // broken script is accepted unchecked rather than reported.
    let python_checked = cfg!(feature = "python");
    assert_eq!(
        syntax_warnings.len(),
        1 + usize::from(python_checked),
        "Expected syntax warnings for executor script (and computation script \
         when the python feature is on): {syntax_warnings:#?}"
    );
    assert_eq!(
        syntax_warnings
            .iter()
            .any(|d| d.message.contains("calc.py")),
        python_checked,
        "Finding for calc.py should follow the python feature"
    );
    assert!(
        syntax_warnings
            .iter()
            .any(|d| d.message.contains("executor.sh")),
        "Expected finding for executor.sh"
    );
}
