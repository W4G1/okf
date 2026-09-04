# okf-validator

Conformance validation, multi-language syntax checking, and opinionated linting for [Open Knowledge Format
(OKF)](https://github.com/GoogleCloudPlatform/open-knowledge-format) v0.2
bundles, built on [okf-core](https://crates.io/crates/okf-core).

- `validate_bundle` checks conformance with severity-tagged diagnostics:
  only true spec violations are errors; material data integrity issues,
  temporal inconsistencies, broken links/references, and code-block / computation script syntax issues are surfaced as warnings and infos.
- `lint_bundle` evaluates opinionated bundle hygiene and formatting checks,
  each finding tagged with a stable rule code (`L1`..`L12`) so CI can pin or silence individual rules.
- `check_syntax` provides direct syntax validation for code blocks and scripts across
  Python, JavaScript, TypeScript, Rust, SQL, JSON, YAML, and Bash.

Most users get this crate through [okf](https://crates.io/crates/okf), which
re-exports it alongside okf-core and ships the `okf` command-line tool
(`okf validate`, `okf lint`).

## Cargo features

Conformance validation and linting need no third-party parser. The four
language parsers behind the syntax checks (V24, V25) are optional, each
behind a feature of the same name, and all on by default:

| Feature      | Languages              | Parser crate        | Notes |
|--------------|------------------------|---------------------|-------|
| `python`     | Python                 | `rustpython-parser` | 60+ crates; depends on the **LGPL-3.0-only** `malachite` crates, plus `tiny-keccak` (CC0-1.0), `unicode_names2` (Unicode-DFS-2016), and unmaintained `unic-*` crates |
| `javascript` | JavaScript, TypeScript | `oxc_parser`        | |
| `rust`       | Rust                   | `syn`               | |
| `sql`        | SQL                    | `sqlparser`         | |

JSON, YAML, and Bash checking is built in and always available.

With a feature disabled, `check_syntax` accepts that language unchecked
(returning `Ok(())`, as for an unknown tag), the validator emits no syntax
diagnostics for it, and `Language::is_supported` reports `false`. So a
consumer under an allow-list licence policy such as `cargo deny` can keep
the whole conformance and lint surface, and SQL checking, while dropping the
copyleft tree:

```toml
[dependencies]
okf-validator = { version = "0.2", default-features = false, features = ["sql"] }
```
