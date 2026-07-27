//! Error types for the crate.

use crate::yaml::YamlError;
use std::fmt;

/// Errors raised when parsing or validating a single OKF concept document.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DocumentError {
    /// The frontmatter opened with `---` but no closing `---` was found.
    UnterminatedFrontmatter,
    /// The frontmatter block did not contain a YAML mapping.
    FrontmatterNotMapping,
    /// The YAML frontmatter could not be parsed.
    InvalidYaml(YamlError),
    /// Required frontmatter keys are missing or empty.
    MissingKeys(Vec<String>),
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
        }
    }
}

impl std::error::Error for DocumentError {}

impl From<YamlError> for DocumentError {
    fn from(e: YamlError) -> Self {
        Self::InvalidYaml(e)
    }
}

/// Errors raised when loading or operating on a bundle on disk.
#[derive(Debug)]
pub enum BundleError {
    /// An I/O error occurred while reading the bundle.
    Io(std::io::Error),
    /// The bundle root does not exist or is not a directory.
    NotADirectory(std::path::PathBuf),
    /// A concept document failed to parse.
    Document {
        /// Path to the offending file.
        path: std::path::PathBuf,
        /// The underlying document error.
        error: DocumentError,
    },
    /// A path could not be turned into a valid concept id.
    InvalidConceptId {
        /// Path that produced the error.
        path: std::path::PathBuf,
        /// Description of why it was invalid.
        reason: String,
    },
}

impl fmt::Display for BundleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "I/O error: {e}"),
            Self::NotADirectory(p) => {
                write!(f, "bundle root is not a directory: {}", p.display())
            }
            Self::Document { path, error } => {
                write!(f, "{}: {error}", path.display())
            }
            Self::InvalidConceptId { path, reason } => {
                write!(f, "{}: invalid concept id ({reason})", path.display())
            }
        }
    }
}

impl std::error::Error for BundleError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            Self::Document { error, .. } => Some(error),
            _ => None,
        }
    }
}

impl From<std::io::Error> for BundleError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}
