//! # okf: the Open Knowledge Format, in pure Rust
//!
//! A dependency-free implementation of the [Open Knowledge Format (OKF)
//! v0.2][spec], Google's open, human- and agent-friendly format for
//! representing knowledge as a directory of markdown files with YAML
//! frontmatter.
//!
//! This crate is the user-facing entry point of the OKF workspace. It ships
//! the `okf` command-line tool (`cargo install okf`) and re-exports the entire
//! API of the [`okf-core`](https://docs.rs/okf-core) crate, where the
//! specification is implemented, so as a dependency it is used exactly like a
//! plain library:
//!
//! ```no_run
//! use okf::{Bundle, validate_bundle};
//!
//! let bundle = Bundle::load("./my_bundle")?;
//! println!("{} concepts", bundle.len());
//!
//! let report = validate_bundle(&bundle);
//! if report.is_conformant() {
//!     println!("conformant OKF v0.2 bundle");
//! }
//! # Ok::<(), okf::BundleError>(())
//! ```
//!
//! See the okf-core documentation for the full model: [`Bundle`], [`Document`],
//! [`Frontmatter`], [`ConceptId`], the provenance/trust/attestation families,
//! and the [`validate_bundle`] conformance checker.
//!
//! [spec]: https://github.com/GoogleCloudPlatform/open-knowledge-format/blob/main/SPEC.md

#![forbid(unsafe_code)]
#![warn(missing_docs)]
#![warn(clippy::pedantic, clippy::nursery)]

/// Compiles and runs the `README.md` examples as doctests.
///
/// `cfg(doctest)` means this item exists only while `cargo test` collects
/// doctests, so it never reaches the public API or the rendered documentation.
/// Without it the README's Rust blocks would be prose that nothing checks.
#[cfg(doctest)]
#[doc = include_str!("../../../README.md")]
pub struct ReadmeExamples;

pub use okf_core::*;
pub use okf_validator::*;
