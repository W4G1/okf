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
