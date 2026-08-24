use std::process::Command;

fn okf_bin() -> &'static str {
    env!("CARGO_BIN_EXE_okf")
}

#[test]
fn cli_validate_flags_code_block_syntax_warning_in_bundle() {
    let temp_dir = std::env::temp_dir().join(format!("okf_val_code_err_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&temp_dir);
    std::fs::create_dir_all(&temp_dir).unwrap();

    let comp_md = r#"---
type: Attested Computation
title: Bad Calc
description: Calculation with syntax error
generated: { by: agent/test, at: 2026-06-20T22:53:05Z }
verified: { by: human:walter, at: 2026-06-25T09:00:00Z }
runtime: python
parameters:
  - { name: x, type: integer }
executor:
  name: runner
  resource: run.sh
  receipt: [run.log]
attester:
  name: checker
  resource: verify.py
---

# Bad Calc

Calculation description.

# Computation

```python
def broken_calc(
```
"#;

    std::fs::write(temp_dir.join("bad_calc.md"), comp_md).unwrap();
    std::fs::write(temp_dir.join("run.sh"), "echo run\n").unwrap();
    std::fs::write(temp_dir.join("verify.py"), "def check(): pass\n").unwrap();
    std::fs::write(
        temp_dir.join("index.md"),
        "# Index\n\n* [Bad Calc](bad_calc.md)\n",
    )
    .unwrap();

    let output = Command::new(okf_bin())
        .arg("validate")
        .arg(&temp_dir)
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("[warning]"),
        "stdout should have warning: {stdout}"
    );
    assert!(
        stdout.contains("syntax check failed"),
        "stdout should mention syntax failure: {stdout}"
    );
    // Warnings do not fail conformance
    assert_eq!(output.status.code(), Some(0));

    let _ = std::fs::remove_dir_all(&temp_dir);
}

#[test]
fn cli_validate_flags_computation_script_syntax_warning_in_bundle() {
    let temp_dir = std::env::temp_dir().join(format!("okf_val_script_err_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&temp_dir);
    std::fs::create_dir_all(&temp_dir).unwrap();

    let comp_md = r#"---
type: Attested Computation
title: External Script Calc
description: Calculation with external script syntax error
generated: { by: agent/test, at: 2026-06-20T22:53:05Z }
verified: { by: human:walter, at: 2026-06-25T09:00:00Z }
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

# External Script Calc

Calculation description.
"#;

    std::fs::write(temp_dir.join("calc.md"), comp_md).unwrap();
    std::fs::write(temp_dir.join("calc.py"), "def broken_code(\n").unwrap();
    std::fs::write(temp_dir.join("executor.sh"), "echo run\n").unwrap();
    std::fs::write(temp_dir.join("attester.py"), "def verify(): pass\n").unwrap();
    std::fs::write(
        temp_dir.join("index.md"),
        "# Index\n\n* [External Script Calc](calc.md)\n",
    )
    .unwrap();

    let output = Command::new(okf_bin())
        .arg("validate")
        .arg(&temp_dir)
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("[warning]"),
        "stdout should have warning: {stdout}"
    );
    assert!(
        stdout.contains("syntax check failed"),
        "stdout should mention syntax failure: {stdout}"
    );
    // Warnings do not fail conformance
    assert_eq!(output.status.code(), Some(0));

    let _ = std::fs::remove_dir_all(&temp_dir);
}
