//! `okf`: a command-line tool for the Open Knowledge Format.
//!
//! Subcommands:
//!   validate     <bundle>   Check a bundle against OKF v0.2 §11 conformance.
//!   info         <bundle>   Print a summary of a bundle.
//!   trust        <bundle>   Report trust tier, status, and staleness per concept.
//!   computations <bundle>   List Attested Computation contracts.
//!   index        <bundle>   (Re)generate every index.md in a bundle.
//!   graph        <bundle>   Print the cross-link graph (text, or DOT with --dot).
//!   parse        <file>     Parse one concept document and print its structure.
//!   fmt          <file>     Normalize a document by parse + re-serialize.
//!
//! Argument parsing is hand-rolled to keep the crate dependency-free.

use okf::{validate_bundle_at, Bundle, Date, Document, Severity, TrustTier, Value};
use std::collections::BTreeMap;
use std::path::Path;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!("{USAGE}");
        return ExitCode::FAILURE;
    }
    let cmd = args[0].as_str();
    let rest = &args[1..];

    let result = match cmd {
        "validate" => cmd_validate(rest),
        "info" => cmd_info(rest),
        "trust" => cmd_trust(rest),
        "computations" => cmd_computations(rest),
        "index" => cmd_index(rest),
        "graph" => cmd_graph(rest),
        "parse" => cmd_parse(rest),
        "fmt" => cmd_fmt(rest),
        "-h" | "--help" | "help" => {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        "-V" | "--version" | "version" => {
            println!(
                "okf {} (OKF spec v{})",
                env!("CARGO_PKG_VERSION"),
                okf::OKF_VERSION
            );
            return ExitCode::SUCCESS;
        }
        other => {
            eprintln!("unknown subcommand: {other}\n\n{USAGE}");
            return ExitCode::FAILURE;
        }
    };

    match result {
        Ok(code) => code,
        Err(msg) => {
            eprintln!("error: {msg}");
            ExitCode::FAILURE
        }
    }
}

const USAGE: &str = "\
okf: Open Knowledge Format toolkit

USAGE:
    okf <command> [args]

COMMANDS:
    validate     <bundle>    Check a bundle against OKF v0.2 conformance (§11)
    info         <bundle>    Summarize a bundle (concepts, types, trust, links)
    trust        <bundle>    Report trust tier, status, and staleness per concept
    computations <bundle>    List Attested Computation contracts (§10)
    index        <bundle>    (Re)generate every index.md in the bundle
    graph        <bundle>    Print the cross-link graph (--dot for Graphviz DOT)
    parse        <file>      Parse one concept document and print its structure
    fmt          <file>      Normalize a document by parse + re-serialize (-w writes)

OPTIONS:
    -h, --help               Show this help
    -V, --version            Show version
        --today <YYYY-MM-DD> Evaluate staleness against this date instead of today";

/// Flags that consume the argument after them, so it is not mistaken for a
/// positional path.
const VALUED_FLAGS: [&str; 1] = ["--today"];

/// Returns the first positional argument, or an error. Everything after a `--`
/// separator is treated as positional (so paths beginning with `-` work).
fn positional<'a>(args: &'a [String], what: &str) -> Result<&'a str, String> {
    if let Some(pos) = args.iter().position(|a| a == "--") {
        if let Some(arg) = args.get(pos + 1) {
            return Ok(arg.as_str());
        }
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
    Err(format!("missing {what}"))
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
fn today(args: &[String]) -> Result<Option<Date>, String> {
    let uses_flag = args
        .iter()
        .any(|a| a == "--today" || a.starts_with("--today="));
    if !uses_flag {
        return Ok(Date::today_utc());
    }
    let raw = flag_value(args, "--today").ok_or("--today needs a YYYY-MM-DD date")?;
    Date::parse(raw)
        .map(Some)
        .ok_or_else(|| format!("--today is not a YYYY-MM-DD date: {raw}"))
}

fn load(path: &str) -> Result<Bundle, String> {
    Bundle::load(path).map_err(|e| e.to_string())
}

fn cmd_validate(args: &[String]) -> Result<ExitCode, String> {
    let path = positional(args, "<bundle>")?;
    let bundle = load(path)?;
    let report = validate_bundle_at(&bundle, today(args)?);

    for d in &report.diagnostics {
        println!("{d}");
    }

    let errors = report.error_count();
    let warnings = report.warning_count();
    let infos = report.of(Severity::Info).count();
    println!(
        "\n{} concept(s); {errors} error(s), {warnings} warning(s), {infos} info.",
        bundle.len()
    );

    if report.is_conformant() {
        println!("✓ conformant with OKF v{}", okf::OKF_VERSION);
        Ok(ExitCode::SUCCESS)
    } else {
        println!("✗ not conformant with OKF v{}", okf::OKF_VERSION);
        Ok(ExitCode::FAILURE)
    }
}

fn cmd_info(args: &[String]) -> Result<ExitCode, String> {
    let path = positional(args, "<bundle>")?;
    let bundle = load(path)?;

    println!("bundle:      {}", bundle.root().display());
    println!(
        "okf_version: {}",
        bundle
            .okf_version()
            .unwrap_or_else(|| "(undeclared)".to_string())
    );
    println!("concepts:    {}", bundle.len());
    println!("index.md:    {}", bundle.index_files().len());
    println!("log.md:      {}", bundle.log_files().len());

    let mut by_type: BTreeMap<String, usize> = BTreeMap::new();
    for c in bundle.concepts() {
        let t = c.type_().unwrap_or_else(|| "(none)".to_string());
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
    println!("\ntrust tiers (§5.3):");
    for (tier, n) in &by_tier {
        println!("  {n:>4}  {tier}");
    }
    println!("\nstatus (§5.4):");
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

fn cmd_trust(args: &[String]) -> Result<ExitCode, String> {
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
            let target = match &resolved.concept {
                Some(id) => format!(" -> {id}"),
                None => String::new(),
            };
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

fn cmd_computations(args: &[String]) -> Result<ExitCode, String> {
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

fn cmd_index(args: &[String]) -> Result<ExitCode, String> {
    let path = positional(args, "<bundle>")?;
    let written = okf::index::regenerate_indexes(path).map_err(|e| e.to_string())?;
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

fn cmd_graph(args: &[String]) -> Result<ExitCode, String> {
    let path = positional(args, "<bundle>")?;
    let dot = has_flag(args, "--dot");
    let sources = has_flag(args, "--sources");
    let bundle = load(path)?;

    if dot {
        println!("digraph okf {{");
        println!("  rankdir=LR; node [shape=box, fontsize=10];");
        for c in bundle.concepts() {
            for link in bundle.links_from(&c.id) {
                let style = if link.exists {
                    ""
                } else {
                    " [style=dashed, color=red]"
                };
                println!(
                    "  {:?} -> {:?}{style};",
                    c.id.to_string(),
                    link.target.to_string()
                );
            }
            if sources {
                for target in bundle.derived_from(&c.id) {
                    println!(
                        "  {:?} -> {:?} [style=dotted, color=blue, label=\"source\"];",
                        c.id.to_string(),
                        target.to_string()
                    );
                }
            }
        }
        println!("}}");
    } else {
        for c in bundle.concepts() {
            let links = bundle.links_from(&c.id);
            let derived: Vec<&okf::ConceptId> = if sources {
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
    Ok(ExitCode::SUCCESS)
}

fn cmd_parse(args: &[String]) -> Result<ExitCode, String> {
    let path = positional(args, "<file>")?;
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let doc = Document::parse(&text).map_err(|e| e.to_string())?;
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

    println!("\ntrust (§5):");
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

    let sources = fm.sources();
    if !sources.is_empty() {
        println!("\nsources ({}) (§5.1):", sources.len());
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

    let attributions = doc.attributions();
    if !attributions.is_empty() {
        println!("\nattribution ({}) (§5.1):", attributions.len());
        for a in &attributions {
            let target = match &a.source {
                Some(source) => source.label().to_string(),
                None => "(no matching sources[].id)".to_string(),
            };
            println!("  [^{}] x{} -> {target}", a.label, a.references);
        }
    }

    if let Some(contract) = doc.attested_computation() {
        println!("\nattested computation (§10):");
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
            "\nlegacy citations ({}), superseded by `sources` (§13.1):",
            citations.len()
        );
        for cit in &citations {
            println!("  [{}] {}", cit.number, cit.raw);
        }
    }
    if conformant {
        Ok(ExitCode::SUCCESS)
    } else {
        Ok(ExitCode::FAILURE)
    }
}

/// Prints a frontmatter block one key per line.
///
/// v0.2 frontmatter nests (`generated`, `sources`, `executor`), so collection
/// values are printed as an indented block under their key rather than run
/// together on the key's line.
fn print_frontmatter(fm: &okf::Frontmatter) {
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

fn cmd_fmt(args: &[String]) -> Result<ExitCode, String> {
    let path = positional(args, "<file>")?;
    let write = has_flag(args, "-w") || has_flag(args, "--write");
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let doc = Document::parse(&text).map_err(|e| e.to_string())?;
    let out = doc.serialize();

    if write {
        std::fs::write(Path::new(path), &out).map_err(|e| e.to_string())?;
        println!("formatted {path}");
    } else {
        print!("{out}");
    }
    Ok(ExitCode::SUCCESS)
}
