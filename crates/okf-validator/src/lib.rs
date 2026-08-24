//! # okf-validator: conformance checking and linting for OKF bundles
//!
//! Companion to [`okf-core`](https://docs.rs/okf-core), the pure-Rust
//! implementation of the [Open Knowledge Format (OKF) v0.2][spec]. This crate
//! judges bundles; okf-core models them.
//!
//! - [`validate`] checks conformance: [`validate_bundle`] reports true spec
//!   violations as [`Severity::Error`], and material data integrity issues,
//!   temporal checks, broken references, contract discrepancies, and script syntax
//!   errors as [`Severity::Warning`] or [`Severity::Info`].
//! - [`lint`] is the opinionated companion: [`lint_bundle`] evaluates 12
//!   bundle formatting, structure, and authoring hygiene rules, each finding tagged with a stable rule code (`L1`..`L12`).
//! - [`syntax`] provides in-process syntax checking for Python,
//!   JavaScript, TypeScript, Rust, SQL, JSON, YAML, and Bash.
//!
//! Staleness checks depend on the wall clock, so they are opt-in via
//! [`validate_bundle_at`] and [`lint_bundle_at`], which take the date to
//! compare against; the plain variants are deterministic.
//!
//! Most users get this crate through the [`okf`](https://docs.rs/okf) crate,
//! which re-exports it alongside okf-core and ships the `okf` CLI.
//!
//! ```no_run
//! use okf_core::Bundle;
//! use okf_validator::validate_bundle;
//!
//! let bundle = Bundle::load("./my_bundle")?;
//! let report = validate_bundle(&bundle);
//! if report.is_conformant() {
//!     println!("conformant OKF v0.2 bundle");
//! }
//! # Ok::<(), okf_core::BundleError>(())
//! ```
//!
//! [spec]: https://github.com/GoogleCloudPlatform/open-knowledge-format/blob/main/SPEC.md

#![forbid(unsafe_code)]
#![warn(missing_docs)]
// Pedantic and nursery lints keep the published crate tidy; the few cases
// where a lint is genuinely wrong for this codebase are silenced inline with a
// justification.
#![warn(clippy::pedantic, clippy::nursery)]

pub mod lint;
pub mod syntax;
pub mod validate;

#[doc(inline)]
pub use lint::{lint_bundle, lint_bundle_at};
#[doc(inline)]
pub use syntax::{
    FencedCodeBlock, Language, SyntaxError, check_syntax, extract_fenced_code_blocks,
};
#[doc(inline)]
pub use validate::{Diagnostic, Report, Severity, validate_bundle, validate_bundle_at};
