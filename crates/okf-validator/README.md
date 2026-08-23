# okf-validator

Conformance validation and opinionated linting for [Open Knowledge Format
(OKF)](https://github.com/GoogleCloudPlatform/open-knowledge-format) v0.2
bundles, built on [okf-core](https://crates.io/crates/okf-core).

- `validate_bundle` checks conformance with severity-tagged diagnostics:
  only true spec violations are errors, optional-family problems are warnings
  and infos.
- `lint_bundle` goes beyond conformance with bundle hygiene checks, each
  finding tagged with a stable rule code so CI can pin or silence individual
  rules.

Most users get this crate through [okf](https://crates.io/crates/okf), which
re-exports it alongside okf-core and ships the `okf` command-line tool
(`okf validate`, `okf lint`).
