//! Scaffolding and initialization utilities for OKF bundles and concept documents.
//!
//! - [`init_bundle`] initializes a compliant OKF bundle structure (`index.md`,
//!   `log.md`, and an optional sample concept).
//! - [`create_concept`] generates a new concept `.md` document with appropriate
//!   frontmatter and markdown structure.

use crate::date::{Date, DateTime};
use crate::document::Document;
use crate::frontmatter::Frontmatter;
use crate::trust::Status;
use crate::yaml::{Mapping, Value};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Options for initializing a new OKF bundle.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BundleInitOptions {
    /// Bundle title for the root index.
    pub title: String,
    /// Whether to create a sample concept (e.g. `overview.md`).
    pub create_sample: bool,
    /// The name/id of the sample concept (default "overview").
    pub sample_name: String,
    /// Author attribution for generated frontmatter (e.g. "human:alice").
    pub author: Option<String>,
    /// Whether to overwrite existing files if they already exist.
    pub force: bool,
}

impl Default for BundleInitOptions {
    fn default() -> Self {
        Self {
            title: "Knowledge Base".to_string(),
            create_sample: true,
            sample_name: "overview".to_string(),
            author: None,
            force: false,
        }
    }
}

/// Options for creating a new concept document.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConceptOptions {
    /// Concept type (e.g. "Concept", "Metric", "Attested Computation", "Playbook").
    pub type_: String,
    /// Concept title (default derived from file name).
    pub title: Option<String>,
    /// Concept description.
    pub description: Option<String>,
    /// Lifecycle status.
    pub status: Status,
    /// Author attribution (e.g. "human:alice").
    pub author: Option<String>,
    /// Whether this concept is an Attested Computation contract.
    pub attested: bool,
    /// Optional tags.
    pub tags: Vec<String>,
    /// Whether to overwrite existing file if it already exists.
    pub force: bool,
}

impl Default for ConceptOptions {
    fn default() -> Self {
        Self {
            type_: "Concept".to_string(),
            title: None,
            description: None,
            status: Status::Draft,
            author: None,
            attested: false,
            tags: Vec::new(),
            force: false,
        }
    }
}

/// Returns a default author actor string based on system user environment
/// variables or `"human:author"`.
#[must_use]
pub fn default_author() -> String {
    if let Ok(author) = std::env::var("OKF_AUTHOR")
        && !author.trim().is_empty()
    {
        return author.trim().to_string();
    }
    for env_key in &["USER", "USERNAME", "LOGNAME"] {
        if let Ok(user) = std::env::var(env_key) {
            let clean: String = user
                .chars()
                .filter(|c| c.is_alphanumeric() || *c == '_' || *c == '-')
                .collect();
            if !clean.is_empty() {
                return format!("human:{clean}");
            }
        }
    }
    "human:author".to_string()
}

/// Capitalizes and formats a file stem into a display title.
#[must_use]
pub fn title_from_name(name: &str) -> String {
    let name = name.trim_end_matches(".md");
    let mut words = Vec::new();
    for part in name.split(['_', '-']) {
        if part.is_empty() {
            continue;
        }
        let mut chars = part.chars();
        let first = chars.next().unwrap_or_default().to_uppercase().to_string();
        words.push(format!("{first}{}", chars.as_str()));
    }
    if words.is_empty() {
        "Untitled".to_string()
    } else {
        words.join(" ")
    }
}

/// Formats the current UTC instant as an ISO-8601 string.
#[must_use]
pub fn current_iso_timestamp() -> String {
    DateTime::now_utc().map_or_else(
        || "2026-01-01T00:00:00Z".to_string(),
        |dt| {
            let clean = DateTime {
                date: dt.date,
                hour: dt.hour,
                minute: dt.minute,
                second: dt.second,
                nanosecond: 0,
                offset_minutes: Some(0),
                has_time: true,
            };
            clean.to_string()
        },
    )
}

/// Builds a scaffolded [`Document`] matching the provided options.
#[must_use]
pub fn build_concept_document(options: &ConceptOptions, title: &str) -> Document {
    let mut fm = Frontmatter::new();

    let is_attested =
        options.attested || options.type_ == crate::computation::ATTESTED_COMPUTATION_TYPE;
    let actual_type = if is_attested {
        crate::computation::ATTESTED_COMPUTATION_TYPE
    } else {
        options.type_.as_str()
    };
    fm.set("type", Value::String(actual_type.to_string()));
    fm.set("title", Value::String(title.to_string()));

    let desc = options
        .description
        .as_deref()
        .filter(|d| !d.trim().is_empty())
        .map_or_else(
            || format!("Overview and details for {title}."),
            ToString::to_string,
        );
    fm.set("description", Value::String(desc));
    fm.set("status", Value::String(options.status.to_string()));

    let author = options.author.clone().unwrap_or_else(default_author);
    let mut gen_map = Mapping::new();
    gen_map.insert("by", Value::String(author));
    gen_map.insert("at", Value::String(current_iso_timestamp()));
    fm.set("generated", Value::Mapping(gen_map));

    if !options.tags.is_empty() {
        let tag_values: Vec<Value> = options
            .tags
            .iter()
            .map(|t| Value::String(t.clone()))
            .collect();
        fm.set("tags", Value::Sequence(tag_values));
    }

    if is_attested {
        fm.set("runtime", Value::String("python".to_string()));

        let mut param_map = Mapping::new();
        param_map.insert("name", Value::String("input_data".to_string()));
        param_map.insert("type", Value::String("string".to_string()));
        param_map.insert("required", Value::Bool(true));
        fm.set(
            "parameters",
            Value::Sequence(vec![Value::Mapping(param_map)]),
        );

        let mut exec_map = Mapping::new();
        exec_map.insert(
            "resource",
            Value::String("references/skills/run.md".to_string()),
        );
        exec_map.insert(
            "receipt",
            Value::Sequence(vec![Value::String("result".to_string())]),
        );
        fm.set("executor", Value::Mapping(exec_map));

        let mut att_map = Mapping::new();
        att_map.insert(
            "resource",
            Value::String("references/attesters/verify.py".to_string()),
        );
        fm.set("attester", Value::Mapping(att_map));

        let body = format!(
            "# {title}\n\n# Computation\n\n```python\n# Sanctioned computation logic\n```\n"
        );
        Document::new(fm, body)
    } else {
        let body = format!("# {title}\n\nDescribe {title} here.\n");
        Document::new(fm, body)
    }
}

/// Creates a new concept markdown file with proper frontmatter and heading.
///
/// # Errors
///
/// Returns an [`io::Error`] if the file already exists (and `force` is false),
/// or if writing to disk fails.
pub fn create_concept(path: impl AsRef<Path>, options: &ConceptOptions) -> io::Result<PathBuf> {
    let mut path = path.as_ref().to_path_buf();
    if path.extension().is_none() {
        path.set_extension("md");
    }

    if path.exists() && !options.force {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("concept file already exists: {}", path.display()),
        ));
    }

    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent)?;
    }

    let file_stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("concept");
    let title = options
        .title
        .clone()
        .unwrap_or_else(|| title_from_name(file_stem));

    let doc = build_concept_document(options, &title);
    fs::write(&path, doc.serialize())?;
    Ok(path)
}

/// Initializes a new OKF bundle at `root` with `index.md`, `log.md`, and
/// optionally an initial concept.
///
/// Returns the list of created file paths on success.
///
/// # Errors
///
/// Returns an [`io::Error`] if `index.md` or `log.md` already exists (and
/// `force` is false), or if writing to disk fails.
pub fn init_bundle(
    root: impl AsRef<Path>,
    options: &BundleInitOptions,
) -> io::Result<Vec<PathBuf>> {
    let root = root.as_ref();
    fs::create_dir_all(root)?;

    let index_path = root.join("index.md");
    let log_path = root.join("log.md");

    if !options.force {
        if index_path.exists() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("index.md already exists: {}", index_path.display()),
            ));
        }
        if log_path.exists() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("log.md already exists: {}", log_path.display()),
            ));
        }
    }

    let mut created = Vec::new();

    let sample_rel = if options.create_sample {
        let sample_stem = if options.sample_name.trim().is_empty() {
            "overview"
        } else {
            options.sample_name.trim().trim_end_matches(".md")
        };
        let sample_path = root.join(format!("{sample_stem}.md"));
        let title = title_from_name(sample_stem);
        let desc = format!("Initial {} concept for this bundle.", title.to_lowercase());
        let concept_opts = ConceptOptions {
            type_: "Concept".to_string(),
            title: Some(title.clone()),
            description: Some(desc.clone()),
            status: Status::Draft,
            author: options.author.clone(),
            attested: false,
            tags: Vec::new(),
            force: options.force,
        };
        create_concept(&sample_path, &concept_opts)?;
        created.push(sample_path);
        Some((title, format!("{sample_stem}.md"), desc))
    } else {
        None
    };

    // Build index.md
    let index_text = if let Some((title, link, desc)) = sample_rel {
        format!(
            "---\nokf_version: \"{}\"\n---\n\n# Concept\n\n* [{title}]({link}) - {desc}\n",
            crate::OKF_VERSION
        )
    } else {
        format!(
            "---\nokf_version: \"{}\"\n---\n\n# {}\n",
            crate::OKF_VERSION,
            options.title
        )
    };
    fs::write(&index_path, index_text)?;
    created.push(index_path);

    // Build log.md
    let today = Date::today_utc().unwrap_or(Date {
        year: 2026,
        month: 1,
        day: 1,
    });
    let log_text = format!("# Update Log\n\n## {today}\n* **Creation**: Initialized OKF bundle.\n");
    fs::write(&log_path, log_text)?;
    created.push(log_path);

    Ok(created)
}
