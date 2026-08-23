//! `okf`: a command-line tool for the Open Knowledge Format.
//!
//! Subcommands:
//! ```text
//!   init         [dir]      Initialize a new OKF bundle (--title, --bare).
//!   new          <path>     Create a new concept document (--type, --title, --attested).
//!   validate     <bundle>   Check a bundle against OKF v0.2 conformance.
//!   info         <bundle>   Print a summary of a bundle.
//!   trust        <bundle>   Report trust tier, status, and staleness per concept.
//!   links        <bundle>   Inspect internal, broken, and external cross-links.
//!   computations <bundle>   List Attested Computation contracts.
//!   index        <bundle>   (Re)generate every index.md in a bundle.
//!   graph        <bundle>   Print the cross-link graph (text, mermaid, or json).
//!   parse        <file>     Parse one concept document and print its structure.
//!   fmt          <path>     Normalize document(s) by parse + re-serialize (-w writes).
//!   lint         <bundle>   Opinionated bundle health checks.
//!   diff         <a> <b>    OKF-semantics diff between two bundles.
//! ```
//!
//! Argument parsing is hand-rolled to keep the crate dependency-free.

#![warn(clippy::pedantic, clippy::nursery)]

use crate::{
    Bundle, BundleInitOptions, ConceptOptions, Date, Document, FixOptions, Link, Severity,
    TrustTier, Value, bundle_diff, create_concept, init_bundle, lint_bundle_at, remediate_bundle,
    validate_bundle_at,
};
use std::collections::BTreeMap;
use std::fmt::Write as _;
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
    init         [dir]       Initialize a new OKF bundle (--title, --bare)
    new          <path>      Create a new concept document (--type, --title, --attested)
    validate     <bundle>    Check a bundle against OKF v0.2 conformance (--fix)
    info         <bundle>    Summarize a bundle (concepts, types, trust, links)
    trust        <bundle>    Report trust tier, status, and staleness per concept
    links        <bundle>    Inspect internal, broken, and external cross-links
    computations <bundle>    List Attested Computation contracts
    index        <bundle>    (Re)generate every index.md in the bundle
    graph        <bundle>    Print the cross-link graph (--format text|mermaid|json)
    parse        <file>      Parse one concept document and print its structure
    fmt          <path>      Normalize document(s) by parse + re-serialize (-w writes)
    lint         <bundle>    Opinionated bundle health checks (--fix)
    diff         <a> <b>     OKF-semantics diff between two bundles

OPTIONS:
    -h, --help               Show this help
    -V, --version            Show version
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

    if fix {
        let options = FixOptions::validation_only(author);
        let fix_report = remediate_bundle(path, &options)
            .map_err(|e| CliError::data(format!("could not apply fixes: {e}")))?;
        let (written, regenerated) = fix_report
            .apply()
            .map_err(|e| CliError::data(format!("could not write fixes: {e}")))?;
        if written > 0 || !regenerated.is_empty() {
            println!(
                "Applied {} fix(es) across {} file(s).\n",
                fix_report.total_remediations(),
                written
            );
        }
    }

    let bundle = load(path)?;
    let report = validate_bundle_at(&bundle, today(args)?);

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

fn cmd_lint(args: &[String]) -> Result<ExitCode, CliError> {
    let path = positional(args, "<bundle>")?;
    let fix = has_flag(args, "--fix");
    let author = flag_value(args, "--author").map(ToString::to_string);

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
        if written > 0 || !regenerated.is_empty() {
            println!(
                "Applied {} fix(es) across {} file(s).\n",
                fix_report.total_remediations(),
                written
            );
        }
    }

    let bundle = load(path)?;
    let report = lint_bundle_at(&bundle, today(args)?);

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

fn cmd_trust(args: &[String]) -> Result<ExitCode, CliError> {
    let path = positional(args, "<bundle>")?;
    let bundle = load(path)?;
    let today = today(args)?;

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

fn cmd_computations(args: &[String]) -> Result<ExitCode, CliError> {
    let path = positional(args, "<bundle>")?;
    let bundle = load(path)?;

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

fn cmd_index(args: &[String]) -> Result<ExitCode, CliError> {
    let path = positional(args, "<bundle>")?;
    if !Path::new(path).is_dir() {
        return Err(CliError::no_input(format!(
            "bundle root is not a directory: {path}"
        )));
    }
    let written =
        crate::index::regenerate_indexes(path).map_err(|e| CliError::no_input(e.to_string()))?;
    if written.is_empty() {
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
    let mut out = String::new();
    out.push_str("{\n");
    out.push_str("  \"concepts\": [\n");

    let concepts = bundle.concepts();
    for (i, c) in concepts.iter().enumerate() {
        let _ = writeln!(out, "    {{");
        let _ = writeln!(out, "      \"id\": {},", json_str(&c.id.to_string()));

        let links = bundle.links_from(&c.id);
        out.push_str("      \"links\": [");
        if links.is_empty() {
            out.push_str("],\n");
        } else {
            out.push('\n');
            for (j, link) in links.iter().enumerate() {
                let _ = writeln!(out, "        {{");
                let _ = writeln!(
                    out,
                    "          \"target\": {},",
                    json_str(&link.target.to_string())
                );
                let _ = writeln!(out, "          \"exists\": {},", link.exists);
                let _ = writeln!(out, "          \"text\": {},", json_str(&link.text));
                let _ = writeln!(out, "          \"raw\": {}", json_str(&link.raw));
                out.push_str(if j + 1 == links.len() {
                    "        }\n"
                } else {
                    "        },\n"
                });
            }
            out.push_str("      ],\n");
        }

        if sources {
            let derived: Vec<String> = bundle
                .derived_from(&c.id)
                .iter()
                .map(|t| json_str(&t.to_string()))
                .collect();
            out.push_str("      \"sources\": [");
            out.push_str(&derived.join(", "));
            out.push_str("]\n");
        } else {
            out.push_str("      \"sources\": []\n");
        }

        out.push_str(if i + 1 == concepts.len() {
            "    }\n"
        } else {
            "    },\n"
        });
    }
    out.push_str("  ]\n");
    out.push_str("}\n");
    print!("{out}");
}

/// A minimal JSON string escaper (RFC 8259). The crate is zero-dependency,
/// so the escaping is hand-rolled rather than delegating to `serde_json`.
fn json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn cmd_parse(args: &[String]) -> Result<ExitCode, CliError> {
    let path = positional(args, "<file>")?;
    let text = std::fs::read_to_string(path).map_err(|e| CliError::no_input(e.to_string()))?;
    let doc = Document::parse(&text).map_err(|e| CliError::data(e.to_string()))?;
    let fm = &doc.frontmatter;

    println!("frontmatter ({} key(s)):", fm.as_mapping().len());
    print_frontmatter(fm);
    let conformant = doc.validate().is_ok();
    println!("\nhas non-empty `type`: {conformant}");
    println!("body: {} byte(s)", doc.body.len());
    let missing = doc.missing_recommended();
    if !missing.is_empty() {
        println!("missing recommended: {}", missing.join(", "));
    }

    print_parse_trust(fm);
    print_parse_sources(fm);
    print_parse_attributions(&doc);
    print_parse_computation(&doc);
    print_parse_links(&doc);

    if conformant {
        Ok(ExitCode::SUCCESS)
    } else {
        Ok(ExitCode::from(EX_DATAERR))
    }
}

/// The trust block: tier, status, `generated`, `verified`, and `stale_after`.
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

fn cmd_fmt(args: &[String]) -> Result<ExitCode, CliError> {
    let path = positional(args, "<path>")?;
    let write = has_flag(args, "-w") || has_flag(args, "--write");
    let target_path = Path::new(path);

    if !target_path.exists() {
        return Err(CliError::no_input(format!(
            "No such file or directory: {path}"
        )));
    }

    if target_path.is_dir() {
        let mut md_files = Vec::new();
        collect_markdown_files(target_path, &mut md_files)?;
        md_files.sort();

        if md_files.is_empty() {
            println!("no markdown files found in {}", target_path.display());
            return Ok(ExitCode::SUCCESS);
        }

        let mut formatted_count = 0;
        let mut error_count = 0;

        for file_path in &md_files {
            let text = match std::fs::read_to_string(file_path) {
                Ok(t) => t,
                Err(e) => {
                    eprintln!("error reading {}: {e}", file_path.display());
                    error_count += 1;
                    continue;
                }
            };
            let doc = match Document::parse(&text) {
                Ok(d) => d,
                Err(e) => {
                    eprintln!("error parsing {}: {e}", file_path.display());
                    error_count += 1;
                    continue;
                }
            };
            let out = doc.serialize();
            if write {
                if let Err(e) = std::fs::write(file_path, &out) {
                    eprintln!("error writing {}: {e}", file_path.display());
                    error_count += 1;
                    continue;
                }
                println!("formatted {}", file_path.display());
            } else {
                println!("--- {} ---", file_path.display());
                print!("{out}");
            }
            formatted_count += 1;
        }

        if write {
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
        let doc = Document::parse(&text).map_err(|e| CliError::data(e.to_string()))?;
        let out = doc.serialize();

        if write {
            std::fs::write(target_path, &out).map_err(|e| CliError::no_input(e.to_string()))?;
            println!("formatted {path}");
        } else {
            print!("{out}");
        }
        Ok(ExitCode::SUCCESS)
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

    let options = BundleInitOptions {
        title: title.to_string(),
        create_sample: !bare,
        sample_name: sample_name.to_string(),
        author,
        force,
    };

    let created = init_bundle(dir, &options)
        .map_err(|e| CliError::data(format!("could not initialize bundle: {e}")))?;

    println!("initialized OKF bundle at {dir}");
    for p in &created {
        println!("  created {}", p.display());
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

    let options = ConceptOptions {
        type_: type_.to_string(),
        title,
        description,
        status,
        author,
        attested,
        tags: Vec::new(),
        force,
    };

    let created = create_concept(&target_path, &options)
        .map_err(|e| CliError::data(format!("could not create concept: {e}")))?;

    println!("created concept at {}", created.display());
    Ok(ExitCode::SUCCESS)
}

fn cmd_links(args: &[String]) -> Result<ExitCode, CliError> {
    let path = positional(args, "<bundle>")?;
    let bundle = load(path)?;
    let broken_only = has_flag(args, "--broken");
    let check = has_flag(args, "--check");
    let show_external = has_flag(args, "--all") || has_flag(args, "--external");
    let format = flag_value(args, "--format").unwrap_or("text");

    match format {
        "text" => Ok(print_links_text(&bundle, broken_only, show_external, check)),
        "json" => Ok(print_links_json(&bundle, broken_only, show_external, check)),
        other => Err(CliError::usage(format!(
            "unknown --format: {other} (expected text|json)"
        ))),
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

    let mut out = String::new();
    out.push_str("{\n");
    let _ = writeln!(out, "  \"concepts_count\": {},", bundle.len());
    let _ = writeln!(out, "  \"broken_count\": {broken_count},");
    out.push_str("  \"concepts\": [\n");

    let concepts = bundle.concepts();
    let mut first_c = true;
    for c in concepts {
        let links = bundle.links_from(&c.id);
        let doc_links = c.document.links();
        let ext_links: Vec<&Link> = doc_links
            .iter()
            .filter(|l| l.kind == crate::LinkKind::External)
            .collect();

        if broken_only && !links.iter().any(|l| !l.exists) {
            continue;
        }

        if !first_c {
            out.push_str(",\n");
        }
        first_c = false;

        let _ = writeln!(out, "    {{");
        let _ = writeln!(out, "      \"id\": {},", json_str(&c.id.to_string()));
        out.push_str("      \"links\": [");

        let mut first_l = true;
        for l in links {
            if broken_only && l.exists {
                continue;
            }
            if !first_l {
                out.push(',');
            }
            first_l = false;
            out.push('\n');
            let _ = writeln!(out, "        {{");
            let _ = writeln!(
                out,
                "          \"target\": {},",
                json_str(&l.target.to_string())
            );
            let _ = writeln!(out, "          \"raw\": {},", json_str(&l.raw));
            let _ = writeln!(out, "          \"exists\": {},", l.exists);
            let _ = writeln!(out, "          \"kind\": \"internal\",");
            let _ = writeln!(out, "          \"text\": {}", json_str(&l.text));
            out.push_str("        }");
        }

        if show_external && !broken_only {
            for ext in ext_links {
                if !first_l {
                    out.push(',');
                }
                first_l = false;
                out.push('\n');
                let _ = writeln!(out, "        {{");
                let _ = writeln!(out, "          \"target\": {},", json_str(&ext.target));
                let _ = writeln!(out, "          \"raw\": {},", json_str(&ext.target));
                let _ = writeln!(out, "          \"exists\": true,");
                let _ = writeln!(out, "          \"kind\": \"external\",");
                let _ = writeln!(out, "          \"text\": {}", json_str(&ext.text));
                out.push_str("        }");
            }
        }

        if first_l {
            out.push_str("]\n");
        } else {
            out.push('\n');
            out.push_str("      ]\n");
        }
        out.push_str("    }");
    }
    out.push_str("\n  ]\n}\n");
    print!("{out}");

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
