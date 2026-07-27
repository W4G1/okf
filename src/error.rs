//! Error types for the crate.

use crate::concept_id::ConceptIdError;
use crate::yaml::YamlError;
use std::fmt;

/// Errors raised when parsing or validating a single OKF concept document.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum DocumentError {
    /// The frontmatter opened with `---` but no closing `---` was found.
    UnterminatedFrontmatter,
    /// The frontmatter block did not contain a YAML mapping.
    FrontmatterNotMapping,
    /// The YAML frontmatter could not be parsed.
    InvalidYaml(YamlError),
    /// Required frontmatter keys are missing or empty.
    MissingKeys(Vec<String>),
    /// The file's path could not be turned into a valid concept id.
    InvalidConceptId(ConceptIdError),
}

impl fmt::Display for DocumentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnterminatedFrontmatter => {
                write!(f, "Unterminated YAML frontmatter block")
            }
            Self::FrontmatterNotMapping => {
                write!(f, "Frontmatter must be a YAML mapping")
            }
            Self::InvalidYaml(e) => write!(f, "Invalid YAML in frontmatter: {e}"),
            Self::MissingKeys(keys) => {
                write!(f, "Missing required frontmatter keys: {}", keys.join(", "))
            }
            Self::InvalidConceptId(e) => write!(f, "Invalid concept id: {e}"),
        }
    }
}

impl std::error::Error for DocumentError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidYaml(e) => Some(e),
            Self::InvalidConceptId(e) => Some(e),
            _ => None,
        }
    }
}

impl From<YamlError> for DocumentError {
    fn from(e: YamlError) -> Self {
        Self::InvalidYaml(e)
    }
}

impl From<ConceptIdError> for DocumentError {
    fn from(e: ConceptIdError) -> Self {
        Self::InvalidConceptId(e)
    }
}

/// Errors raised when loading or operating on a bundle on disk.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum BundleError {
    /// An I/O error occurred while reading the bundle.
    ///
    /// The original [`std::io::Error`] is not `Clone`, so the kind and the
    /// rendered message are stored instead. The [`std::error::Error::source`]
    /// chain is therefore unavailable for this variant; use
    /// [`BundleError::io_kind`] to inspect the failure category.
    Io {
        /// The kind of I/O failure (`NotFound`, `PermissionDenied`, ...).
        kind: std::io::ErrorKind,
        /// The rendered message of the original error, which preserves any
        /// path context the OS or stdlib attached.
        message: String,
    },
    /// The bundle root does not exist or is not a directory.
    NotADirectory(std::path::PathBuf),
    /// A concept document failed to parse.
    Document {
        /// Path to the offending file.
        path: std::path::PathBuf,
        /// The underlying document error.
        error: DocumentError,
    },
}

impl BundleError {
    /// The [`std::io::ErrorKind`] for an [`BundleError::Io`] variant, or `None`
    /// for any other variant. Convenient for matching on the failure category
    /// without downcasting.
    #[must_use]
    pub const fn io_kind(&self) -> Option<std::io::ErrorKind> {
        match self {
            Self::Io { kind, .. } => Some(*kind),
            _ => None,
        }
    }
}

impl fmt::Display for BundleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { message, .. } => write!(f, "I/O error: {message}"),
            Self::NotADirectory(p) => {
                write!(f, "bundle root is not a directory: {}", p.display())
            }
            Self::Document { path, error } => {
                write!(f, "{}: {error}", path.display())
            }
        }
    }
}

impl std::error::Error for BundleError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Document { error, .. } => Some(error),
            _ => None,
        }
    }
}

impl From<std::io::Error> for BundleError {
    fn from(e: std::io::Error) -> Self {
        Self::Io {
            kind: e.kind(),
            message: e.to_string(),
        }
    }
}
