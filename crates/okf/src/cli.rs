//! `okf`: a command-line tool for the Open Knowledge Format.
//!
//! Subcommands:
//! ```text
//!   init         [dir]      Initialize a new OKF bundle (--title, --bare, --json).
//!   new          <path>     Create a new concept document (--type, --title, --attested, --json).
//!   validate     <bundle>   Check a bundle against OKF v0.2 conformance (--fix, --json).
//!   info         <bundle>   Print a summary of a bundle (--json).
//!   trust        <bundle>   Report trust tier, status, and staleness per concept (--json).
//!   links        <bundle>   Inspect internal, broken, and external cross-links (--json).
//!   computations <bundle>   List Attested Computation contracts (--json).
//!   index        <bundle>   (Re)generate every index.md in a bundle (--json).
//!   graph        <bundle>   Print the cross-link graph (--format text|mermaid|json, --json).
//!   parse        <file>     Parse one concept document and print its structure (--json).
//!   fmt          <path>     Normalize document(s) by parse + re-serialize (-w writes, --check verifies).
//!   lint         <bundle>   Opinionated bundle health checks (--fix, --json).
//!   diff         <a> <b>    OKF-semantics diff between two bundles (--json).
//! ```
//!
//! Argument parsing is hand-rolled to keep the crate dependency-free.

#![warn(clippy::pedantic, clippy::nursery)]

use crate::{
    Bundle, BundleInitOptions, ConceptOptions, Date, Document, DocumentError, FixOptions, Link,
    Report, Severity, TrustTier, Value, bundle_diff, create_concept, init_bundle, lint_bundle_at,
    remediate_bundle, validate_bundle_at,
};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

// Exit codes follow the sysexits.h convention so CI can tell apart a missing
// bundle from a malformed one. The numeric values are stable: EX_USAGE marks a
// bad command line, EX_DATAERR marks incorrect input data, EX_NOINPUT marks an
// input that could not be opened.
const EX_USAGE: u8 = 2;
const EX_DATAERR: u8 = 65;
const EX_NOINPUT: u8 = 66;

/// A command-line error paired with the exit code it should produce.
struct CliError {
    message: String,
    code: u8,
}

impl CliError {
    fn usage(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            code: EX_USAGE,
        }
    }

    fn data(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            code: EX_DATAERR,
        }
    }

    fn no_input(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            code: EX_NOINPUT,
        }
    }
}

/// Runs the `okf` CLI on `args` (the program name already stripped) and
/// returns the process exit code.
///
/// This is the whole CLI: the `okf` binary and the `cargo okf` subcommand
/// (the `cargo-okf` crate) are both thin wrappers around it.
#[must_use]
pub fn run(args: &[String]) -> ExitCode {
    if args.is_empty() {
        eprintln!("{USAGE}");
        return ExitCode::from(EX_USAGE);
    }
    let cmd = args[0].as_str();
    let rest = &args[1..];

    let result = match cmd {
        "init" => cmd_init(rest),
        "new" => cmd_new(rest),
        "validate" => cmd_validate(rest),
        "info" => cmd_info(rest),
        "trust" => cmd_trust(rest),
        "links" => cmd_links(rest),
        "computations" => cmd_computations(rest),
        "index" => cmd_index(rest),
        "graph" => cmd_graph(rest),
        "parse" => cmd_parse(rest),
        "fmt" => cmd_fmt(rest),
        "lint" => cmd_lint(rest),
        "diff" => cmd_diff(rest),
        "-h" | "--help" | "help" => {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        "-V" | "--version" | "version" => {
            println!(
                "okf {} (OKF spec v{})",
                env!("CARGO_PKG_VERSION"),
                crate::OKF_VERSION
            );
            return ExitCode::SUCCESS;
        }
        other => {
            eprintln!("unknown subcommand: {other}\n\n{USAGE}");
            return ExitCode::from(EX_USAGE);
        }
    };

    match result {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {}", e.message);
            ExitCode::from(e.code)
        }
    }
}

const USAGE: &str = "\
okf: Open Knowledge Format toolkit

USAGE:
    okf <command> [args]

COMMANDS:
    init         [dir]       Initialize a new OKF bundle (--title, --bare, --json)
    new          <path>      Create a new concept document (--type, --title, --attested, --json)
    validate     <bundle>    Check a bundle against OKF v0.2 conformance (--fix, --json)
    info         <bundle>    Summarize a bundle (concepts, types, trust, links, --json)
    trust        <bundle>    Report trust tier, status, and staleness per concept (--json)
    links        <bundle>    Inspect internal, broken, and external cross-links (--json)
    computations <bundle>    List Attested Computation contracts (--json)
    index        <bundle>    (Re)generate every index.md in the bundle (--json)
    graph        <bundle>    Print the cross-link graph (--format text|mermaid|json, --json)
    parse        <file>      Parse one concept document and print its structure (--json)
    fmt          <path>      Normalize document(s) by parse + re-serialize (-w writes, --check verifies)
    lint         <bundle>    Opinionated bundle health checks (--fix, --json)
    diff         <a> <b>     OKF-semantics diff between two bundles (--json)

OPTIONS:
    -h, --help               Show this help
    -V, --version            Show version
    -j, --json               Output results as JSON (or --format json)
        --today <YYYY-MM-DD> Evaluate staleness against this date instead of today";

/// Flags that consume the argument after them, so it is not mistaken for a
/// positional path.
const VALUED_FLAGS: [&str; 8] = [
    "--today",
    "--format",
    "--type",
    "--title",
    "--description",
    "--author",
    "--sample-name",
    "--status",
];

/// Returns the first positional argument, or an error. Everything after a `--`
/// separator is treated as positional (so paths beginning with `-` work).
fn positional<'a>(args: &'a [String], what: &str) -> Result<&'a str, CliError> {
    if let Some(pos) = args.iter().position(|a| a == "--")
        && let Some(arg) = args.get(pos + 1)
    {
        return Ok(arg.as_str());
    }
    let mut skip = false;
    for arg in args {
        if std::mem::replace(&mut skip, false) {
            continue;
        }
        if VALUED_FLAGS.contains(&arg.as_str()) {
            skip = true;
        } else if !arg.starts_with('-') {
            return Ok(arg.as_str());
        }
    }
    Err(CliError::usage(format!("missing {what}")))
}

/// All positional arguments, in order. Flags and their values are skipped, and
/// everything after a `--` separator is treated as positional.
fn positionals(args: &[String]) -> Vec<&str> {
    if let Some(pos) = args.iter().position(|a| a == "--") {
        return args[pos + 1..].iter().map(String::as_str).collect();
    }
    let mut out = Vec::new();
    let mut skip = false;
    for arg in args {
        if std::mem::replace(&mut skip, false) {
            continue;
        }
        if VALUED_FLAGS.contains(&arg.as_str()) {
            skip = true;
        } else if !arg.starts_with('-') {
            out.push(arg.as_str());
        }
    }
    out
}

fn has_flag(args: &[String], flag: &str) -> bool {
    args.iter().any(|a| a == flag)
}

/// The value of `--flag <value>` or `--flag=<value>`.
fn flag_value<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
    let prefix = format!("{flag}=");
    for (i, arg) in args.iter().enumerate() {
        if let Some(value) = arg.strip_prefix(&prefix) {
            return Some(value);
        }
        if arg == flag {
            return args.get(i + 1).map(String::as_str);
        }
    }
    None
}

/// Checks if JSON output mode was requested via `--json`, `-j`, or `--format json`.
fn is_json(args: &[String]) -> Result<bool, CliError> {
    if has_flag(args, "--json") || has_flag(args, "-j") {
        return Ok(true);
    }
    match flag_value(args, "--format") {
        None | Some("text") => Ok(false),
        Some("json") => Ok(true),
        Some(other) => Err(CliError::usage(format!(
            "unknown --format: {other} (expected text|json)"
        ))),
    }
}

/// The date staleness is evaluated against: `--today YYYY-MM-DD`, else the
/// system clock in UTC.
fn today(args: &[String]) -> Result<Option<Date>, CliError> {
    let uses_flag = args
        .iter()
        .any(|a| a == "--today" || a.starts_with("--today="));
    if !uses_flag {
        return Ok(Date::today_utc());
    }
    let raw = flag_value(args, "--today")
        .ok_or_else(|| CliError::usage("--today needs a YYYY-MM-DD date"))?;
    Date::parse(raw)
        .map(Some)
        .ok_or_else(|| CliError::usage(format!("--today is not a YYYY-MM-DD date: {raw}")))
}

fn load(path: &str) -> Result<Bundle, CliError> {
    Bundle::load(path).map_err(|e| CliError::no_input(e.to_string()))
}

fn cmd_validate(args: &[String]) -> Result<ExitCode, CliError> {
    let path = positional(args, "<bundle>")?;
    let fix = has_flag(args, "--fix");
    let author = flag_value(args, "--author").map(ToString::to_string);
    let json = is_json(args)?;

    let mut applied_fixes = 0;
    let mut written_files = 0;

    if fix {
        let options = FixOptions::validation_only(author);
        let fix_report = remediate_bundle(path, &options)
            .map_err(|e| CliError::data(format!("could not apply fixes: {e}")))?;
        let (written, regenerated) = fix_report
            .apply()
            .map_err(|e| CliError::data(format!("could not write fixes: {e}")))?;
        applied_fixes = fix_report.total_remediations();
        written_files = written;
        if !json && (written > 0 || !regenerated.is_empty()) {
            println!("Applied {applied_fixes} fix(es) across {written} file(s).\n");
        }
    }

    let bundle = load(path)?;
    let report = validate_bundle_at(&bundle, today(args)?);

    if json {
        print_validate_json(&bundle, &report, fix, applied_fixes, written_files);
        if report.is_conformant() {
            Ok(ExitCode::SUCCESS)
        } else {
            Ok(ExitCode::from(EX_DATAERR))
        }
    } else {
        for d in &report.diagnostics {
            print_diagnostic(d);
        }

        let errors = report.error_count();
        let warnings = report.warning_count();
        let infos = report.of(Severity::Info).count();
        let fixable = report.fixable_count();

        if fixable > 0 {
            println!(
                "\n{} concept(s); {errors} error(s), {warnings} warning(s) ({fixable} fixable with `--fix`), {infos} info.",
                bundle.len()
            );
        } else {
            println!(
                "\n{} concept(s); {errors} error(s), {warnings} warning(s), {infos} info.",
                bundle.len()
            );
        }

        if report.is_conformant() {
            println!("✓ conformant with OKF v{}", crate::OKF_VERSION);
            Ok(ExitCode::SUCCESS)
        } else {
            println!("✗ not conformant with OKF v{}", crate::OKF_VERSION);
            Ok(ExitCode::from(EX_DATAERR))
        }
    }
}

fn print_validate_json(
    bundle: &Bundle,
    report: &Report,
    fix: bool,
    fixes_applied: usize,
    files_written: usize,
) {
    let diagnostics: Vec<serde_json::Value> = report
        .diagnostics
        .iter()
        .map(|d| {
            serde_json::json!({
                "severity": d.severity.to_string(),
                "message": strip_spec_references(&d.message),
                "path": d.path.as_ref().map(|p| p.to_string_lossy()),
                "concept": d.concept.as_ref().map(ToString::to_string),
                "fixable": d.fixable,
            })
        })
        .collect();

    let mut val = serde_json::json!({
        "okf_version": crate::OKF_VERSION,
        "bundle": bundle.root().to_string_lossy(),
        "conformant": report.is_conformant(),
        "concepts_count": bundle.len(),
        "error_count": report.error_count(),
        "warning_count": report.warning_count(),
        "info_count": report.of(Severity::Info).count(),
        "fixable_count": report.fixable_count(),
        "diagnostics": diagnostics,
    });

    if fix {
        val["fixes_applied"] = serde_json::json!(fixes_applied);
        val["files_written"] = serde_json::json!(files_written);
    }

    println!("{}", serde_json::to_string_pretty(&val).unwrap_or_default());
}

fn cmd_lint(args: &[String]) -> Result<ExitCode, CliError> {
    let path = positional(args, "<bundle>")?;
    let fix = has_flag(args, "--fix");
    let author = flag_value(args, "--author").map(ToString::to_string);
    let json = is_json(args)?;

    let mut applied_fixes = 0;
    let mut written_files = 0;

    if fix {
        let options = FixOptions {
            author,
            ..Default::default()
        };
        let fix_report = remediate_bundle(path, &options)
            .map_err(|e| CliError::data(format!("could not apply fixes: {e}")))?;
        let (written, regenerated) = fix_report
            .apply()
            .map_err(|e| CliError::data(format!("could not write fixes: {e}")))?;
        applied_fixes = fix_report.total_remediations();
        written_files = written;
        if !json && (written > 0 || !regenerated.is_empty()) {
            println!("Applied {applied_fixes} fix(es) across {written} file(s).\n");
        }
    }

    let bundle = load(path)?;
    let report = lint_bundle_at(&bundle, today(args)?);

    if json {
        print_lint_json(&bundle, &report, fix, applied_fixes, written_files);
        let warnings = report.warning_count();
        if warnings == 0 {
            Ok(ExitCode::SUCCESS)
        } else {
            Ok(ExitCode::from(EX_DATAERR))
        }
    } else {
        for d in &report.diagnostics {
            print_diagnostic(d);
        }

        let warnings = report.warning_count();
        let infos = report.of(Severity::Info).count();
        let fixable = report.fixable_count();

        if fixable > 0 {
            println!(
                "\n{} concept(s); {warnings} warning(s) ({fixable} fixable with `--fix`), {infos} info.",
                bundle.len()
            );
        } else {
            println!(
                "\n{} concept(s); {warnings} warning(s), {infos} info.",
                bundle.len()
            );
        }

        if warnings == 0 {
            println!("✓ clean lint");
            Ok(ExitCode::SUCCESS)
        } else if fixable > 0 {
            println!("✗ {warnings} lint warning(s) ({fixable} fixable with `--fix`)");
            Ok(ExitCode::from(EX_DATAERR))
        } else {
            println!("✗ {warnings} lint warning(s)");
            Ok(ExitCode::from(EX_DATAERR))
        }
    }
}

fn print_lint_json(
    bundle: &Bundle,
    report: &Report,
    fix: bool,
    fixes_applied: usize,
    files_written: usize,
) {
    let diagnostics: Vec<serde_json::Value> = report
        .diagnostics
        .iter()
        .map(|d| {
            serde_json::json!({
                "severity": d.severity.to_string(),
                "message": strip_spec_references(&d.message),
                "path": d.path.as_ref().map(|p| p.to_string_lossy()),
                "concept": d.concept.as_ref().map(ToString::to_string),
                "fixable": d.fixable,
            })
        })
        .collect();

    let mut val = serde_json::json!({
        "okf_version": crate::OKF_VERSION,
        "bundle": bundle.root().to_string_lossy(),
        "clean": report.warning_count() == 0,
        "concepts_count": bundle.len(),
        "warning_count": report.warning_count(),
        "info_count": report.of(Severity::Info).count(),
        "fixable_count": report.fixable_count(),
        "diagnostics": diagnostics,
    });

    if fix {
        val["fixes_applied"] = serde_json::json!(fixes_applied);
        val["files_written"] = serde_json::json!(files_written);
    }

    println!("{}", serde_json::to_string_pretty(&val).unwrap_or_default());
}

/// Prints diagnostics without implementation-specific OKF section citations.
fn print_diagnostic(diagnostic: &crate::validate::Diagnostic) {
    println!("{}", strip_spec_references(&diagnostic.to_string()));
}

/// Removes section citations from CLI text while preserving the surrounding
/// explanation. The full diagnostic message remains available to library
/// consumers through [`Diagnostic::message`](crate::validate::Diagnostic::message).
fn strip_spec_references(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;

    while i < chars.len() {
        if chars[i] == '('
            && let Some(relative_end) = chars[i + 1..].iter().position(|&c| c == ')')
        {
            let end = i + 1 + relative_end;
            if is_section_group(&chars[i + 1..end]) {
                if out.ends_with(' ') {
                    out.pop();
                }
                i = end + 1;
                continue;
            }
        }

        if let Some(end) = section_reference_end(&chars, i) {
            i = end;
            if i < chars.len() && chars[i].is_whitespace() && out.ends_with(' ') {
                i += 1;
            }
            continue;
        }

        out.push(chars[i]);
        i += 1;
    }

    out.trim_end().to_string()
}

fn is_section_group(chars: &[char]) -> bool {
    let mut i = 0;
    loop {
        while i < chars.len() && chars[i].is_whitespace() {
            i += 1;
        }
        let Some(end) = section_reference_end(chars, i) else {
            return false;
        };
        i = end;
        while i < chars.len() && chars[i].is_whitespace() {
            i += 1;
        }
        if i == chars.len() {
            return true;
        }

        if chars[i] == ',' {
            i += 1;
            continue;
        }
        if chars.get(i..i + 2) == Some(&['t', 'o']) {
            i += 2;
            continue;
        }
        return false;
    }
}

fn section_reference_end(chars: &[char], start: usize) -> Option<usize> {
    if chars.get(start) != Some(&'§') {
        return None;
    }

    let mut i = start + 1;
    let digit_start = i;
    while i < chars.len() && chars[i].is_ascii_digit() {
        i += 1;
    }
    if i == digit_start {
        return None;
    }

    while i < chars.len() && chars[i] == '.' {
        i += 1;
        let decimal_start = i;
        while i < chars.len() && chars[i].is_ascii_digit() {
            i += 1;
        }
        if i == decimal_start {
            return None;
        }
    }
    Some(i)
}

fn cmd_info(args: &[String]) -> Result<ExitCode, CliError> {
    let path = positional(args, "<bundle>")?;
    let bundle = load(path)?;
    let json = is_json(args)?;

    if json {
        print_info_json(&bundle, today(args)?);
        return Ok(ExitCode::SUCCESS);
    }

    println!("bundle:      {}", bundle.root().display());
    println!(
        "okf_version: {}",
        bundle.okf_version().unwrap_or("(undeclared)")
    );
    println!("concepts:    {}", bundle.len());
    println!("index.md:    {}", bundle.index_files().len());
    println!("log.md:      {}", bundle.log_files().len());

    let mut by_type: BTreeMap<String, usize> = BTreeMap::new();
    for c in bundle.concepts() {
        let t = c.type_().as_deref().unwrap_or("(none)").to_string();
        *by_type.entry(t).or_default() += 1;
    }
    if !by_type.is_empty() {
        println!("\ntypes:");
        for (t, n) in &by_type {
            println!("  {n:>4}  {t}");
        }
    }

    let mut by_tier: BTreeMap<String, usize> = BTreeMap::new();
    let mut by_status: BTreeMap<String, usize> = BTreeMap::new();
    for c in bundle.concepts() {
        *by_tier.entry(c.trust_tier().to_string()).or_default() += 1;
        *by_status.entry(c.status().to_string()).or_default() += 1;
    }
    println!("\ntrust tiers:");
    for (tier, n) in &by_tier {
        println!("  {n:>4}  {tier}");
    }
    println!("\nstatus:");
    for (status, n) in &by_status {
        println!("  {n:>4}  {status}");
    }

    if let Some(today) = today(args)? {
        let stale = bundle.stale_on(today);
        println!("\nstale on {today}: {} concept(s)", stale.len());
    }

    let with_sources = bundle
        .concepts()
        .iter()
        .filter(|c| !c.sources().is_empty())
        .count();
    let derivation_edges: usize = bundle
        .concepts()
        .iter()
        .map(|c| bundle.derived_from(&c.id).len())
        .sum();
    println!(
        "\nsources:     {with_sources} concept(s) record provenance; \
         {derivation_edges} derivation edge(s) inside the bundle"
    );
    println!("computations: {}", bundle.attested_computations().count());

    let broken = bundle.broken_links();
    let total_links: usize = bundle
        .concepts()
        .iter()
        .map(|c| bundle.links_from(&c.id).len())
        .sum();
    println!(
        "links:       {total_links} internal ({} broken)",
        broken.len()
    );

    let tags = bundle.tags();
    if !tags.is_empty() {
        println!("tags:        {} distinct", tags.len());
    }

    if !bundle.parse_errors().is_empty() {
        println!("\nunparseable files:");
        for (p, e) in bundle.parse_errors() {
            println!("  {}: {e}", p.display());
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn print_info_json(bundle: &Bundle, today_date: Option<Date>) {
    let mut by_type: BTreeMap<String, usize> = BTreeMap::new();
    for c in bundle.concepts() {
        let t = c.type_().as_deref().unwrap_or("(none)").to_string();
        *by_type.entry(t).or_default() += 1;
    }

    let mut by_tier: BTreeMap<String, usize> = BTreeMap::new();
    let mut by_status: BTreeMap<String, usize> = BTreeMap::new();
    for c in bundle.concepts() {
        *by_tier.entry(c.trust_tier().to_string()).or_default() += 1;
        *by_status.entry(c.status().to_string()).or_default() += 1;
    }

    let (stale_count, stale_concepts) = today_date.map_or_else(
        || (0, Vec::new()),
        |today| {
            let stale = bundle.stale_on(today);
            let ids: Vec<String> = stale.iter().map(|c| c.id.to_string()).collect();
            (stale.len(), ids)
        },
    );

    let with_sources = bundle
        .concepts()
        .iter()
        .filter(|c| !c.sources().is_empty())
        .count();
    let derivation_edges: usize = bundle
        .concepts()
        .iter()
        .map(|c| bundle.derived_from(&c.id).len())
        .sum();

    let broken = bundle.broken_links();
    let total_links: usize = bundle
        .concepts()
        .iter()
        .map(|c| bundle.links_from(&c.id).len())
        .sum();

    let parse_errors: Vec<serde_json::Value> = bundle
        .parse_errors()
        .iter()
        .map(|(p, e)| {
            serde_json::json!({
                "path": p.to_string_lossy(),
                "error": e.to_string(),
            })
        })
        .collect();

    let val = serde_json::json!({
        "bundle": bundle.root().to_string_lossy(),
        "okf_version": bundle.okf_version(),
        "concepts_count": bundle.len(),
        "index_files": bundle.index_files().iter().map(|p| p.to_string_lossy()).collect::<Vec<_>>(),
        "log_files": bundle.log_files().iter().map(|p| p.to_string_lossy()).collect::<Vec<_>>(),
        "types": by_type,
        "trust_tiers": by_tier,
        "status": by_status,
        "stale_count": stale_count,
        "stale_concepts": stale_concepts,
        "sources_count": with_sources,
        "derivation_edges_count": derivation_edges,
        "computations_count": bundle.attested_computations().count(),
        "links": {
            "internal": total_links,
            "broken": broken.len(),
        },
        "tags": bundle.tags().keys().collect::<Vec<_>>(),
        "parse_errors": parse_errors,
    });

    println!("{}", serde_json::to_string_pretty(&val).unwrap_or_default());
}

fn cmd_trust(args: &[String]) -> Result<ExitCode, CliError> {
    let path = positional(args, "<bundle>")?;
    let bundle = load(path)?;
    let today = today(args)?;
    let json = is_json(args)?;

    if json {
        print_trust_json(&bundle, today);
        return Ok(ExitCode::SUCCESS);
    }

    for c in bundle.concepts() {
        let fm = &c.document.frontmatter;
        let stale = match today {
            Some(t) if c.is_stale_on(t) => " STALE",
            _ => "",
        };
        println!("{} [{}] {}{stale}", c.id, c.status(), c.trust_tier());

        if let Some(generated) = fm.generated() {
            println!("  generated: {generated}");
        }
        for verification in fm.verified() {
            println!("  verified:  {verification}");
        }
        if let Some(stale_after) = fm.stale_after() {
            println!("  stale_after: {stale_after}");
        }
        for resolved in bundle.sources_of(&c.id) {
            let target = resolved
                .concept
                .as_ref()
                .map_or_else(String::new, |id| format!(" -> {id}"));
            println!("  source:    {}{target}", resolved.source);
        }
    }

    let mut counts: BTreeMap<TrustTier, usize> = BTreeMap::new();
    for c in bundle.concepts() {
        *counts.entry(c.trust_tier()).or_default() += 1;
    }
    println!("\n{} concept(s):", bundle.len());
    for (tier, n) in &counts {
        println!("  {n:>4}  {tier}");
    }
    Ok(ExitCode::SUCCESS)
}

fn print_trust_json(bundle: &Bundle, today_date: Option<Date>) {
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for c in bundle.concepts() {
        *counts.entry(c.trust_tier().to_string()).or_default() += 1;
    }

    let concepts: Vec<serde_json::Value> = bundle
        .concepts()
        .iter()
        .map(|c| {
            let fm = &c.document.frontmatter;
            let is_stale = today_date.is_some_and(|t| c.is_stale_on(t));
            let generated = fm.generated().map(|g| {
                serde_json::json!({
                    "by": g.by.as_ref().map(ToString::to_string),
                    "at": g.at.as_ref().map(ToString::to_string),
                })
            });
            let verified: Vec<serde_json::Value> = fm
                .verified()
                .iter()
                .map(|v| {
                    serde_json::json!({
                        "by": v.by.as_ref().map(ToString::to_string),
                        "at": v.at.as_ref().map(ToString::to_string),
                    })
                })
                .collect();
            let sources: Vec<serde_json::Value> = bundle
                .sources_of(&c.id)
                .iter()
                .map(|s| {
                    serde_json::json!({
                        "id": s.source.id,
                        "resource": s.source.resource,
                        "target_concept": s.concept.as_ref().map(ToString::to_string),
                    })
                })
                .collect();

            serde_json::json!({
                "id": c.id.to_string(),
                "status": c.status().to_string(),
                "trust_tier": c.trust_tier().to_string(),
                "stale": is_stale,
                "stale_after": fm.stale_after().map(|d| d.to_string()),
                "generated": generated,
                "verified": verified,
                "sources": sources,
            })
        })
        .collect();

    let val = serde_json::json!({
        "bundle": bundle.root().to_string_lossy(),
        "concepts_count": bundle.len(),
        "summary": counts,
        "concepts": concepts,
    });

    println!("{}", serde_json::to_string_pretty(&val).unwrap_or_default());
}

fn cmd_computations(args: &[String]) -> Result<ExitCode, CliError> {
    let path = positional(args, "<bundle>")?;
    let bundle = load(path)?;
    let json = is_json(args)?;

    if json {
        print_computations_json(&bundle);
        return Ok(ExitCode::SUCCESS);
    }

    let mut found = 0;
    for c in bundle.attested_computations() {
        let Some(contract) = c.attested_computation() else {
            continue;
        };
        found += 1;
        println!("{} ({})", c.id, c.display_title());
        println!(
            "  runtime:     {}",
            contract.runtime.as_deref().unwrap_or("(missing)")
        );
        println!("  computation: {}", contract.computation);
        if !contract.parameters.is_empty() {
            let params: Vec<String> = contract
                .parameters
                .iter()
                .map(ToString::to_string)
                .collect();
            println!("  parameters:  {}", params.join(", "));
        }
        if let Some(executor) = &contract.executor {
            println!(
                "  executor:    {} (receipt: {})",
                executor.resource.as_deref().unwrap_or("(missing)"),
                if executor.receipt.is_empty() {
                    "(none)".to_string()
                } else {
                    executor.receipt.join(", ")
                }
            );
        }
        if let Some(attester) = &contract.attester {
            println!(
                "  attester:    {}",
                attester.resource.as_deref().unwrap_or("(missing)")
            );
        }
        let used_by = bundle.backlinks(&c.id);
        if !used_by.is_empty() {
            let ids: Vec<String> = used_by.iter().map(ToString::to_string).collect();
            println!("  used by:     {}", ids.join(", "));
        }
    }

    if found == 0 {
        println!(
            "no `Attested Computation` concepts in {}",
            bundle.root().display()
        );
    } else {
        println!("\n{found} attested computation(s).");
    }
    Ok(ExitCode::SUCCESS)
}

fn print_computations_json(bundle: &Bundle) {
    let comps: Vec<serde_json::Value> = bundle
        .attested_computations()
        .filter_map(|c| {
            let contract = c.attested_computation()?;
            let parameters: Vec<serde_json::Value> = contract
                .parameters
                .iter()
                .map(|p| {
                    serde_json::json!({
                        "name": p.name,
                        "type": p.type_,
                        "required": p.required,
                    })
                })
                .collect();
            let executor = contract.executor.as_ref().map(|exec| {
                serde_json::json!({
                    "resource": exec.resource,
                    "receipt": exec.receipt,
                })
            });
            let attester = contract.attester.as_ref().map(|att| {
                serde_json::json!({
                    "resource": att.resource,
                })
            });
            let used_by: Vec<String> = bundle
                .backlinks(&c.id)
                .iter()
                .map(ToString::to_string)
                .collect();

            Some(serde_json::json!({
                "id": c.id.to_string(),
                "title": c.display_title(),
                "runtime": contract.runtime,
                "computation": contract.computation.to_string(),
                "parameters": parameters,
                "executor": executor,
                "attester": attester,
                "used_by": used_by,
            }))
        })
        .collect();

    let val = serde_json::json!({
        "bundle": bundle.root().to_string_lossy(),
        "computations_count": comps.len(),
        "computations": comps,
    });

    println!("{}", serde_json::to_string_pretty(&val).unwrap_or_default());
}

fn cmd_index(args: &[String]) -> Result<ExitCode, CliError> {
    let path = positional(args, "<bundle>")?;
    if !Path::new(path).is_dir() {
        return Err(CliError::no_input(format!(
            "bundle root is not a directory: {path}"
        )));
    }
    let json = is_json(args)?;
    let written =
        crate::index::regenerate_indexes(path).map_err(|e| CliError::no_input(e.to_string()))?;
    if json {
        let val = serde_json::json!({
            "bundle": path,
            "regenerated_count": written.len(),
            "regenerated": written.iter().map(|p| p.to_string_lossy()).collect::<Vec<_>>(),
        });
        println!("{}", serde_json::to_string_pretty(&val).unwrap_or_default());
    } else if written.is_empty() {
        println!("no index files written (empty bundle?)");
    } else {
        for p in &written {
            println!("wrote {}", p.display());
        }
        println!("\n{} index file(s) regenerated.", written.len());
    }
    Ok(ExitCode::SUCCESS)
}

fn cmd_graph(args: &[String]) -> Result<ExitCode, CliError> {
    let path = positional(args, "<bundle>")?;
    let format = graph_format(args)?;
    let sources = has_flag(args, "--sources");
    let bundle = load(path)?;

    match format {
        GraphFormat::Text => print_graph_text(&bundle, sources),
        GraphFormat::Mermaid => print_graph_mermaid(&bundle, sources),
        GraphFormat::Json => print_graph_json(&bundle, sources),
    }
    Ok(ExitCode::SUCCESS)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum GraphFormat {
    Text,
    Mermaid,
    Json,
}

fn graph_format(args: &[String]) -> Result<GraphFormat, CliError> {
    if has_flag(args, "--json") || has_flag(args, "-j") {
        return Ok(GraphFormat::Json);
    }
    match flag_value(args, "--format") {
        None | Some("text") => Ok(GraphFormat::Text),
        Some("mermaid") => Ok(GraphFormat::Mermaid),
        Some("json") => Ok(GraphFormat::Json),
        Some(other) => Err(CliError::usage(format!(
            "unknown --format: {other} (expected text|mermaid|json)"
        ))),
    }
}

fn print_graph_text(bundle: &Bundle, sources: bool) {
    for c in bundle.concepts() {
        let links = bundle.links_from(&c.id);
        let derived: Vec<&crate::ConceptId> = if sources {
            bundle.derived_from(&c.id)
        } else {
            Vec::new()
        };
        if links.is_empty() && derived.is_empty() {
            continue;
        }
        println!("{}", c.id);
        for link in links {
            let mark = if link.exists { "->" } else { "-x" };
            println!("  {mark} {}", link.target);
        }
        for target in derived {
            println!("  ~> {target} (source)");
        }
    }
}

fn print_graph_mermaid(bundle: &Bundle, sources: bool) {
    println!("flowchart LR");

    let mut node_id: BTreeMap<String, String> = BTreeMap::new();
    let mut order: Vec<String> = Vec::new();
    let mut next: usize = 0;

    for c in bundle.concepts() {
        intern(&c.id.to_string(), &mut node_id, &mut order, &mut next);
        for link in bundle.links_from(&c.id) {
            intern(
                &link.target.to_string(),
                &mut node_id,
                &mut order,
                &mut next,
            );
        }
    }

    for id in &order {
        println!("  {}[\"{}\"]", node_id[id], mermaid_label(id));
    }

    for c in bundle.concepts() {
        let from = c.id.to_string();
        for link in bundle.links_from(&c.id) {
            let target = link.target.to_string();
            if link.exists {
                println!("  {} --> {}", node_id[&from], node_id[&target]);
            } else {
                println!("  {} -.->|broken| {}", node_id[&from], node_id[&target]);
            }
        }
        if sources {
            for target in bundle.derived_from(&c.id) {
                let target = target.to_string();
                println!("  {} -.->|source| {}", node_id[&from], node_id[&target]);
            }
        }
    }
}

/// Assigns `id` a stable Mermaid node name, remembering declaration order.
fn intern(
    id: &str,
    node_id: &mut BTreeMap<String, String>,
    order: &mut Vec<String>,
    next: &mut usize,
) -> String {
    if let Some(name) = node_id.get(id) {
        return name.clone();
    }
    let name = format!("n{next}");
    *next += 1;
    node_id.insert(id.to_string(), name.clone());
    order.push(id.to_string());
    name
}

/// A label safe inside a Mermaid `["..."]` node declaration.
fn mermaid_label(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "#quot;")
        .replace('[', "&#91;")
        .replace(']', "&#93;")
}

fn print_graph_json(bundle: &Bundle, sources: bool) {
    let concepts: Vec<serde_json::Value> = bundle
        .concepts()
        .iter()
        .map(|c| {
            let links: Vec<serde_json::Value> = bundle
                .links_from(&c.id)
                .iter()
                .map(|l| {
                    serde_json::json!({
                        "target": l.target.to_string(),
                        "exists": l.exists,
                        "text": l.text,
                        "raw": l.raw,
                    })
                })
                .collect();
            let source_targets: Vec<String> = if sources {
                bundle
                    .derived_from(&c.id)
                    .iter()
                    .map(ToString::to_string)
                    .collect()
            } else {
                Vec::new()
            };

            serde_json::json!({
                "id": c.id.to_string(),
                "links": links,
                "sources": source_targets,
            })
        })
        .collect();

    let val = serde_json::json!({
        "concepts": concepts,
    });

    println!("{}", serde_json::to_string_pretty(&val).unwrap_or_default());
}

fn cmd_parse(args: &[String]) -> Result<ExitCode, CliError> {
    let path = positional(args, "<file>")?;
    let text = std::fs::read_to_string(path).map_err(|e| CliError::no_input(e.to_string()))?;
    let doc = Document::parse(&text).map_err(|e| CliError::data(e.to_string()))?;
    let json = is_json(args)?;

    if json {
        print_parse_json(&doc, path, today(args)?);
        return Ok(ExitCode::SUCCESS);
    }

    let fm = &doc.frontmatter;

    println!("frontmatter ({} key(s)):", fm.as_mapping().len());
    print_frontmatter(fm);
    let conformant = doc.validate().is_ok();
    println!("\nhas non-empty `type`: {conformant}");
    println!("body: {} byte(s)", doc.body.len());
    let missing = doc.missing_recommended();
    if !missing.is_empty() {
        println!("\nmissing recommended keys: {}", missing.join(", "));
    }
    print_parse_trust(fm);
    print_parse_sources(fm);
    print_parse_attributions(&doc);
    print_parse_computation(&doc);
    print_parse_links(&doc);
    Ok(ExitCode::SUCCESS)
}

fn print_parse_json(doc: &Document, path: &str, today_date: Option<Date>) {
    let fm = &doc.frontmatter;
    let is_stale = today_date.is_some_and(|t| fm.is_stale_on(t));
    let generated = fm.generated().map(|g| {
        serde_json::json!({
            "by": g.by.as_ref().map(ToString::to_string),
            "at": g.at.as_ref().map(ToString::to_string),
        })
    });
    let verified: Vec<serde_json::Value> = fm
        .verified()
        .iter()
        .map(|v| {
            serde_json::json!({
                "by": v.by.as_ref().map(ToString::to_string),
                "at": v.at.as_ref().map(ToString::to_string),
            })
        })
        .collect();
    let sources: Vec<serde_json::Value> = fm
        .sources()
        .iter()
        .map(|s| {
            serde_json::json!({
                "id": s.id,
                "resource": s.resource,
                "title": s.title,
                "resource_kind": format!("{:?}", s.resource_kind()).to_lowercase(),
                "author": s.author.as_ref().map(ToString::to_string),
                "last_modified": s.last_modified.as_ref().map(ToString::to_string),
                "usage_count": s.usage_count,
            })
        })
        .collect();
    let attributions: Vec<serde_json::Value> = doc
        .attributions()
        .iter()
        .map(|a| {
            serde_json::json!({
                "label": a.label,
                "references": a.references,
                "source_id": a.source.as_ref().and_then(|s| s.id.as_deref()),
            })
        })
        .collect();
    let attested_comp = doc.attested_computation().map(|comp| {
        serde_json::json!({
            "runtime": comp.runtime,
            "computation": comp.computation.to_string(),
        })
    });
    let links: Vec<serde_json::Value> = doc
        .links()
        .iter()
        .map(|l| {
            serde_json::json!({
                "text": l.text,
                "target": l.target,
                "kind": format!("{:?}", l.kind).to_lowercase(),
            })
        })
        .collect();

    let val = serde_json::json!({
        "file": path,
        "conformant": doc.validate().is_ok(),
        "missing_recommended": doc.missing_recommended(),
        "type": fm.type_(),
        "title": fm.title(),
        "description": fm.description(),
        "tags": fm.tags(),
        "status": fm.status().to_string(),
        "trust_tier": fm.trust_tier().to_string(),
        "stale": is_stale,
        "stale_after": fm.stale_after().map(|d| d.to_string()),
        "generated": generated,
        "verified": verified,
        "sources": sources,
        "attributions": attributions,
        "attested_computation": attested_comp,
        "links": links,
        "body_bytes": doc.body.len(),
    });

    println!("{}", serde_json::to_string_pretty(&val).unwrap_or_default());
}

/// The `generated` and `verified` blocks, trust tier, and status.
fn print_parse_trust(fm: &crate::Frontmatter) {
    println!("\ntrust:");
    println!("  tier:      {}", fm.trust_tier());
    println!("  status:    {}", fm.status());
    if let Some(generated) = fm.generated() {
        println!("  generated: {generated}");
    }
    for verification in fm.verified() {
        println!("  verified:  {verification}");
    }
    if let Some(stale_after) = fm.stale_after() {
        let note = match Date::today_utc() {
            Some(t) if fm.is_stale_on(t) => " (stale)",
            _ => "",
        };
        println!("  stale_after: {stale_after}{note}");
    }
}

/// The `sources` block with its credibility signals.
fn print_parse_sources(fm: &crate::Frontmatter) {
    let sources = fm.sources();
    if sources.is_empty() {
        return;
    }
    println!("\nsources ({}):", sources.len());
    for source in &sources {
        println!("  {source} [{:?}]", source.resource_kind());
        if let Some(author) = &source.author {
            println!("    author: {author} ({})", author.kind());
        }
        if let Some(count) = source.usage_count {
            let window = source
                .effective_usage_window(fm.usage_window().as_ref())
                .map(|w| format!(" over {w}"))
                .unwrap_or_default();
            println!("    usage_count: {count}{window}");
        }
        if let Some(last_modified) = &source.last_modified {
            println!("    last_modified: {last_modified}");
        }
    }
}

/// Footnote attribution keyed to `sources[].id`.
fn print_parse_attributions(doc: &Document) {
    let attributions = doc.attributions();
    if attributions.is_empty() {
        return;
    }
    println!("\nattribution ({}):", attributions.len());
    for a in &attributions {
        let target = a.source.as_ref().map_or_else(
            || "(no matching sources[].id)".to_string(),
            |source| source.label().to_string(),
        );
        println!("  [^{}] x{} -> {target}", a.label, a.references);
    }
}

/// The Attested Computation contract, when the document carries one.
fn print_parse_computation(doc: &Document) {
    let Some(contract) = doc.attested_computation() else {
        return;
    };
    println!("\nattested computation:");
    println!(
        "  runtime:     {}",
        contract.runtime.as_deref().unwrap_or("(missing)")
    );
    println!("  computation: {}", contract.computation);
    for parameter in &contract.parameters {
        println!("  parameter:   {parameter}");
    }
    if let Some(executor) = &contract.executor {
        println!(
            "  executor:    {}",
            executor.resource.as_deref().unwrap_or("(missing)")
        );
        println!("  receipt:     {}", executor.receipt.join(", "));
    }
    if let Some(attester) = &contract.attester {
        println!(
            "  attester:    {}",
            attester.resource.as_deref().unwrap_or("(missing)")
        );
    }
}

/// Markdown links and any legacy `# Citations` list.
fn print_parse_links(doc: &Document) {
    let links = doc.links();
    if !links.is_empty() {
        println!("\nlinks ({}):", links.len());
        for l in &links {
            println!("  [{:?}] {} -> {}", l.kind, l.text, l.target);
        }
    }
    let citations = doc.citations();
    if !citations.is_empty() {
        println!(
            "\nlegacy citations ({}), superseded by `sources`:",
            citations.len()
        );
        for cit in &citations {
            println!("  [{}] {}", cit.number, cit.raw);
        }
    }
}

/// Prints a frontmatter block one key per line.
///
/// v0.2 frontmatter nests (`generated`, `sources`, `executor`), so collection
/// values are printed as an indented block under their key rather than run
/// together on the key's line.
fn print_frontmatter(fm: &crate::Frontmatter) {
    for (key, value) in fm.as_mapping().iter() {
        let name = scalar(key);
        let nested = matches!(value, Value::Mapping(m) if !m.is_empty())
            || matches!(value, Value::Sequence(s) if !s.is_empty());
        if nested {
            println!("  {name}:");
            for line in value.to_yaml_string().trim_end().lines() {
                println!("    {line}");
            }
        } else {
            println!("  {name}: {}", scalar(value));
        }
    }
}

/// A scalar's text without the trailing newline `Value`'s `Display` adds.
fn scalar(value: &Value) -> String {
    value.to_yaml_string().trim_end().to_string()
}

fn format_markdown_file(file_path: &Path, text: &str) -> Result<String, DocumentError> {
    if file_path.file_name().and_then(|n| n.to_str()) == Some("log.md") {
        Ok(crate::log::Log::parse(text).to_markdown())
    } else {
        let doc = Document::parse(text)?;
        Ok(doc.serialize())
    }
}

#[allow(clippy::too_many_lines)]
fn cmd_fmt(args: &[String]) -> Result<ExitCode, CliError> {
    let path = positional(args, "<path>")?;
    let write = has_flag(args, "-w") || has_flag(args, "--write");
    let check = has_flag(args, "-c") || has_flag(args, "--check");
    let json = is_json(args)?;
    let target_path = Path::new(path);

    if !target_path.exists() {
        return Err(CliError::no_input(format!(
            "No such file or directory: {path}"
        )));
    }

    if check {
        return cmd_fmt_check(target_path, path, json);
    }

    if target_path.is_dir() {
        let mut md_files = Vec::new();
        collect_markdown_files(target_path, &mut md_files)?;
        md_files.sort();

        if md_files.is_empty() {
            if json {
                let val = serde_json::json!({
                    "formatted_count": 0,
                    "error_count": 0,
                    "written": write,
                    "files": Vec::<String>::new(),
                });
                println!("{}", serde_json::to_string_pretty(&val).unwrap_or_default());
            } else {
                println!("no markdown files found in {}", target_path.display());
            }
            return Ok(ExitCode::SUCCESS);
        }

        let mut formatted_count = 0;
        let mut error_count = 0;
        let mut formatted_files = Vec::new();

        for file_path in &md_files {
            let text = match std::fs::read_to_string(file_path) {
                Ok(t) => t,
                Err(e) => {
                    eprintln!("error reading {}: {e}", file_path.display());
                    error_count += 1;
                    continue;
                }
            };
            let out = match format_markdown_file(file_path, &text) {
                Ok(out) => out,
                Err(e) => {
                    eprintln!("error parsing {}: {e}", file_path.display());
                    error_count += 1;
                    continue;
                }
            };
            if write {
                if let Err(e) = std::fs::write(file_path, &out) {
                    eprintln!("error writing {}: {e}", file_path.display());
                    error_count += 1;
                    continue;
                }
                if !json {
                    println!("formatted {}", file_path.display());
                }
            } else if !json {
                println!("--- {} ---", file_path.display());
                print!("{out}");
            }
            formatted_count += 1;
            formatted_files.push(file_path.to_string_lossy().into_owned());
        }

        if json {
            let val = serde_json::json!({
                "formatted_count": formatted_count,
                "error_count": error_count,
                "written": write,
                "files": formatted_files,
            });
            println!("{}", serde_json::to_string_pretty(&val).unwrap_or_default());
        } else if write {
            println!(
                "\n{formatted_count} file(s) formatted in {}.",
                target_path.display()
            );
        }

        if error_count > 0 {
            Ok(ExitCode::from(EX_DATAERR))
        } else {
            Ok(ExitCode::SUCCESS)
        }
    } else {
        let text = std::fs::read_to_string(path).map_err(|e| CliError::no_input(e.to_string()))?;
        let out =
            format_markdown_file(target_path, &text).map_err(|e| CliError::data(e.to_string()))?;

        if write {
            std::fs::write(target_path, &out).map_err(|e| CliError::no_input(e.to_string()))?;
            if json {
                let val = serde_json::json!({
                    "formatted_count": 1,
                    "error_count": 0,
                    "written": true,
                    "file": path,
                });
                println!("{}", serde_json::to_string_pretty(&val).unwrap_or_default());
            } else {
                println!("formatted {path}");
            }
        } else if json {
            let val = serde_json::json!({
                "formatted_count": 1,
                "error_count": 0,
                "written": false,
                "file": path,
            });
            println!("{}", serde_json::to_string_pretty(&val).unwrap_or_default());
        } else {
            print!("{out}");
        }
        Ok(ExitCode::SUCCESS)
    }
}

fn cmd_fmt_check(target_path: &Path, path_str: &str, json: bool) -> Result<ExitCode, CliError> {
    let mut files = Vec::new();
    if target_path.is_dir() {
        collect_markdown_files(target_path, &mut files)?;
        files.sort();
    } else {
        files.push(target_path.to_path_buf());
    }

    if files.is_empty() {
        if json {
            let val = serde_json::json!({
                "clean": true,
                "total_files": 0,
                "unformatted_count": 0,
                "unformatted": Vec::<String>::new(),
            });
            println!("{}", serde_json::to_string_pretty(&val).unwrap_or_default());
        } else {
            println!("no markdown files found in {}", target_path.display());
        }
        return Ok(ExitCode::SUCCESS);
    }

    let mut unformatted = Vec::new();
    let mut parse_errors = 0;

    for file_path in &files {
        let text = match std::fs::read_to_string(file_path) {
            Ok(t) => t,
            Err(e) => {
                eprintln!("error reading {}: {e}", file_path.display());
                parse_errors += 1;
                unformatted.push(file_path.clone());
                continue;
            }
        };
        let formatted = match format_markdown_file(file_path, &text) {
            Ok(f) => f,
            Err(e) => {
                eprintln!("error parsing {}: {e}", file_path.display());
                parse_errors += 1;
                unformatted.push(file_path.clone());
                continue;
            }
        };
        if text != formatted {
            unformatted.push(file_path.clone());
        }
    }

    let clean = unformatted.is_empty() && parse_errors == 0;
    let unformatted_count = unformatted.len();

    if json {
        let unformatted_paths: Vec<String> = unformatted
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        let val = serde_json::json!({
            "clean": clean,
            "total_files": files.len(),
            "unformatted_count": unformatted_count,
            "unformatted": unformatted_paths,
        });
        println!("{}", serde_json::to_string_pretty(&val).unwrap_or_default());
    } else if clean {
        if target_path.is_dir() {
            println!(
                "✓ all {} file(s) formatted in {}",
                files.len(),
                target_path.display()
            );
        } else {
            println!("✓ {} is formatted", target_path.display());
        }
    } else {
        for p in &unformatted {
            println!("needs formatting: {}", p.display());
        }
        if unformatted_count == 1 {
            println!("\n1 file would be reformatted. Run 'okf fmt -w {path_str}' to format.");
        } else {
            println!(
                "\n{unformatted_count} file(s) would be reformatted. Run 'okf fmt -w {path_str}' to format."
            );
        }
    }

    if clean {
        Ok(ExitCode::SUCCESS)
    } else {
        Ok(ExitCode::from(EX_DATAERR))
    }
}

fn collect_markdown_files(dir: &Path, files: &mut Vec<PathBuf>) -> Result<(), CliError> {
    let entries = std::fs::read_dir(dir).map_err(|e| CliError::no_input(e.to_string()))?;
    for entry in entries {
        let entry = entry.map_err(|e| CliError::no_input(e.to_string()))?;
        let path = entry.path();
        if path.is_dir() {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if !name.starts_with('.') && name != "target" && name != "node_modules" {
                collect_markdown_files(&path, files)?;
            }
        } else if path.extension().and_then(|e| e.to_str()) == Some("md") {
            files.push(path);
        }
    }
    Ok(())
}

fn cmd_init(args: &[String]) -> Result<ExitCode, CliError> {
    let pos = positionals(args);
    let dir = pos.first().copied().unwrap_or(".");
    let title = flag_value(args, "--title").unwrap_or("Knowledge Base");
    let bare = has_flag(args, "--bare") || has_flag(args, "--no-sample");
    let sample_name = flag_value(args, "--sample-name").unwrap_or("overview");
    let author = flag_value(args, "--author").map(ToString::to_string);
    let force = has_flag(args, "-f") || has_flag(args, "--force");
    let json = is_json(args)?;

    let options = BundleInitOptions {
        title: title.to_string(),
        create_sample: !bare,
        sample_name: sample_name.to_string(),
        author,
        force,
    };

    let created = init_bundle(dir, &options)
        .map_err(|e| CliError::data(format!("could not initialize bundle: {e}")))?;

    if json {
        let created_paths: Vec<String> = created
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        let val = serde_json::json!({
            "status": "ok",
            "bundle": dir,
            "title": title,
            "created": created_paths,
        });
        println!("{}", serde_json::to_string_pretty(&val).unwrap_or_default());
    } else {
        println!("initialized OKF bundle at {dir}");
        for p in &created {
            println!("  created {}", p.display());
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn cmd_new(args: &[String]) -> Result<ExitCode, CliError> {
    let pos = positionals(args);
    if pos.is_empty() {
        return Err(CliError::usage(
            "missing concept path (usage: okf new <path> or okf new <bundle> <concept-id>)",
        ));
    }
    let target_path = if pos.len() >= 2 {
        Path::new(pos[0]).join(pos[1])
    } else {
        PathBuf::from(pos[0])
    };

    let type_ = flag_value(args, "--type").unwrap_or("Concept");
    let title = flag_value(args, "--title").map(ToString::to_string);
    let description = flag_value(args, "--description").map(ToString::to_string);
    let author = flag_value(args, "--author").map(ToString::to_string);
    let status = match flag_value(args, "--status") {
        Some("stable") => crate::Status::Stable,
        Some("deprecated") => crate::Status::Deprecated,
        Some("draft") | None => crate::Status::Draft,
        Some(other) => crate::Status::Other(other.to_string()),
    };
    let attested = has_flag(args, "--attested");
    let force = has_flag(args, "-f") || has_flag(args, "--force");
    let json = is_json(args)?;

    let options = ConceptOptions {
        type_: type_.to_string(),
        title: title.clone(),
        description,
        status: status.clone(),
        author,
        attested,
        tags: Vec::new(),
        force,
    };

    let created = create_concept(&target_path, &options)
        .map_err(|e| CliError::data(format!("could not create concept: {e}")))?;

    if json {
        let title_val = title.unwrap_or_else(|| {
            created
                .file_stem()
                .and_then(|s| s.to_str())
                .map_or_else(|| "Untitled".to_string(), crate::scaffold::title_from_name)
        });
        let val = serde_json::json!({
            "status": "ok",
            "path": created.to_string_lossy(),
            "type": type_,
            "title": title_val,
            "lifecycle_status": status.to_string(),
            "attested": attested,
        });
        println!("{}", serde_json::to_string_pretty(&val).unwrap_or_default());
    } else {
        println!("created concept at {}", created.display());
    }
    Ok(ExitCode::SUCCESS)
}

fn cmd_links(args: &[String]) -> Result<ExitCode, CliError> {
    let path = positional(args, "<bundle>")?;
    let bundle = load(path)?;
    let broken_only = has_flag(args, "--broken");
    let check = has_flag(args, "--check");
    let show_external = has_flag(args, "--all") || has_flag(args, "--external");
    let json = is_json(args)?;

    if json {
        Ok(print_links_json(&bundle, broken_only, show_external, check))
    } else {
        Ok(print_links_text(&bundle, broken_only, show_external, check))
    }
}

fn print_links_text(
    bundle: &Bundle,
    broken_only: bool,
    show_external: bool,
    check: bool,
) -> ExitCode {
    let broken = bundle.broken_links();

    if broken_only {
        if broken.is_empty() {
            println!(
                "✓ no broken links found ({} concept(s) checked)",
                bundle.len()
            );
            return ExitCode::SUCCESS;
        }
        println!("broken links ({}):\n", broken.len());
        for (source, raw) in &broken {
            println!("  {source} -> {raw}");
        }
        return if check {
            ExitCode::from(EX_DATAERR)
        } else {
            ExitCode::SUCCESS
        };
    }

    let mut total_internal = 0;
    let mut total_external = 0;

    for c in bundle.concepts() {
        let links = bundle.links_from(&c.id);
        let doc_links = c.document.links();
        let external_links: Vec<&Link> = doc_links
            .iter()
            .filter(|l| l.kind == crate::LinkKind::External)
            .collect();

        if links.is_empty() && (!show_external || external_links.is_empty()) {
            continue;
        }

        println!("{}", c.id);
        for link in links {
            total_internal += 1;
            if link.exists {
                println!("  -> {} [ok]", link.target);
            } else {
                println!("  -x {} [broken: {}]", link.target, link.raw);
            }
        }
        if show_external {
            for ext in external_links {
                total_external += 1;
                println!("  => {} [external]", ext.target);
            }
        }
    }

    let broken_count = broken.len();
    if show_external {
        println!(
            "\n{} internal link(s) ({} broken), {} external link(s) across {} concept(s).",
            total_internal,
            broken_count,
            total_external,
            bundle.len()
        );
    } else {
        println!(
            "\n{} internal link(s) ({} broken) across {} concept(s).",
            total_internal,
            broken_count,
            bundle.len()
        );
    }

    if check && broken_count > 0 {
        ExitCode::from(EX_DATAERR)
    } else {
        ExitCode::SUCCESS
    }
}

fn print_links_json(
    bundle: &Bundle,
    broken_only: bool,
    show_external: bool,
    check: bool,
) -> ExitCode {
    let broken = bundle.broken_links();
    let broken_count = broken.len();

    let concepts: Vec<serde_json::Value> = bundle
        .concepts()
        .iter()
        .filter_map(|c| {
            let links = bundle.links_from(&c.id);
            if broken_only && !links.iter().any(|l| !l.exists) {
                return None;
            }

            let mut link_values: Vec<serde_json::Value> = links
                .iter()
                .filter(|l| !broken_only || !l.exists)
                .map(|l| {
                    serde_json::json!({
                        "target": l.target.to_string(),
                        "raw": l.raw,
                        "exists": l.exists,
                        "kind": "internal",
                        "text": l.text,
                    })
                })
                .collect();

            if show_external && !broken_only {
                let doc_links = c.document.links();
                for ext in doc_links
                    .iter()
                    .filter(|l| l.kind == crate::LinkKind::External)
                {
                    link_values.push(serde_json::json!({
                        "target": ext.target,
                        "raw": ext.target,
                        "exists": true,
                        "kind": "external",
                        "text": ext.text,
                    }));
                }
            }

            Some(serde_json::json!({
                "id": c.id.to_string(),
                "links": link_values,
            }))
        })
        .collect();

    let val = serde_json::json!({
        "concepts_count": bundle.len(),
        "broken_count": broken_count,
        "concepts": concepts,
    });

    println!("{}", serde_json::to_string_pretty(&val).unwrap_or_default());

    if check && broken_count > 0 {
        ExitCode::from(EX_DATAERR)
    } else {
        ExitCode::SUCCESS
    }
}

fn cmd_diff(args: &[String]) -> Result<ExitCode, CliError> {
    let paths = positionals(args);
    if paths.len() < 2 {
        return Err(CliError::usage("usage: okf diff <a> <b>"));
    }
    let a = load(paths[0])?;
    let b = load(paths[1])?;
    let diff = bundle_diff(&a, &b);
    let json = is_json(args)?;

    if json {
        print_diff_json(&a, &b, &diff);
        return Ok(ExitCode::SUCCESS);
    }

    println!("{} -> {}", a.root().display(), b.root().display());
    println!("{diff}");

    let changes = diff.added.len()
        + diff.removed.len()
        + diff.renamed.len()
        + diff.content.len()
        + diff.frontmatter.len()
        + diff.trust.len()
        + diff.added_links.len()
        + diff.removed_links.len()
        + diff.mended_links.len()
        + diff.broken_links.len();
    println!("\n{changes} change(s).");
    Ok(ExitCode::SUCCESS)
}

#[allow(clippy::too_many_lines)]
fn print_diff_json(a: &Bundle, b: &Bundle, diff: &crate::BundleDiff) {
    let changes_count = diff.added.len()
        + diff.removed.len()
        + diff.renamed.len()
        + diff.content.len()
        + diff.frontmatter.len()
        + diff.trust.len()
        + diff.added_links.len()
        + diff.removed_links.len()
        + diff.mended_links.len()
        + diff.broken_links.len();

    let renamed: Vec<serde_json::Value> = diff
        .renamed
        .iter()
        .map(|r| {
            serde_json::json!({
                "from": r.from.to_string(),
                "to": r.to.to_string(),
            })
        })
        .collect();

    let frontmatter: Vec<serde_json::Value> = diff
        .frontmatter
        .iter()
        .map(|fc| {
            let changed: Vec<serde_json::Value> = fc
                .changed
                .iter()
                .map(|(k, old, new)| {
                    serde_json::json!({
                        "key": k,
                        "old": old,
                        "new": new,
                    })
                })
                .collect();
            serde_json::json!({
                "concept": fc.id.to_string(),
                "added": fc.added,
                "removed": fc.removed,
                "changed": changed,
            })
        })
        .collect();

    let trust: Vec<serde_json::Value> = diff
        .trust
        .iter()
        .map(|tc| {
            let tier = tc.tier.as_ref().map(|(f, t)| {
                serde_json::json!({
                    "from": f.to_string(),
                    "to": t.to_string(),
                })
            });
            let status = tc.status.as_ref().map(|(f, t)| {
                serde_json::json!({
                    "from": f.to_string(),
                    "to": t.to_string(),
                })
            });
            serde_json::json!({
                "concept": tc.id.to_string(),
                "tier": tier,
                "status": status,
            })
        })
        .collect();

    let added_links: Vec<serde_json::Value> = diff
        .added_links
        .iter()
        .map(|(f, t)| serde_json::json!({ "from": f.to_string(), "target": t.to_string() }))
        .collect();
    let removed_links: Vec<serde_json::Value> = diff
        .removed_links
        .iter()
        .map(|(f, t)| serde_json::json!({ "from": f.to_string(), "target": t.to_string() }))
        .collect();
    let mended_links: Vec<serde_json::Value> = diff
        .mended_links
        .iter()
        .map(|(f, t)| serde_json::json!({ "from": f.to_string(), "target": t }))
        .collect();
    let broken_links: Vec<serde_json::Value> = diff
        .broken_links
        .iter()
        .map(|(f, t)| serde_json::json!({ "from": f.to_string(), "target": t }))
        .collect();

    let val = serde_json::json!({
        "bundle_a": a.root().to_string_lossy(),
        "bundle_b": b.root().to_string_lossy(),
        "changes_count": changes_count,
        "added": diff.added.iter().map(ToString::to_string).collect::<Vec<_>>(),
        "removed": diff.removed.iter().map(ToString::to_string).collect::<Vec<_>>(),
        "renamed": renamed,
        "content_changed": diff.content.iter().map(ToString::to_string).collect::<Vec<_>>(),
        "frontmatter_changed": frontmatter,
        "trust_changed": trust,
        "added_links": added_links,
        "removed_links": removed_links,
        "mended_links": mended_links,
        "broken_links": broken_links,
    });

    println!("{}", serde_json::to_string_pretty(&val).unwrap_or_default());
}
