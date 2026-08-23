//! # okf-validator: conformance checking and linting for OKF bundles
//!
//! Companion to [`okf-core`](https://docs.rs/okf-core), the pure-Rust
//! implementation of the [Open Knowledge Format (OKF) v0.2][spec]. This crate
//! judges bundles; okf-core models them.
//!
//! - [`validate`] checks conformance: [`validate_bundle`] reports only
//!   true spec violations as [`Severity::Error`], with optional-family
//!   problems surfaced as warnings and infos, never as rejections.
//! - [`lint`] is the opinionated companion: [`lint_bundle`] goes beyond
//!   conformance and flags the hygiene issues a continuously-authored corpus
//!   drifts into, each finding tagged with a stable rule code.
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
pub mod validate;

#[doc(inline)]
pub use lint::{lint_bundle, lint_bundle_at};
#[doc(inline)]
pub use validate::{Diagnostic, Report, Severity, validate_bundle, validate_bundle_at};
