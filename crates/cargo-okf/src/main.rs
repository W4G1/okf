//! `cargo okf`: the `okf` CLI as a cargo subcommand.
//!
//! Cargo invokes external subcommands as `cargo-okf okf <args...>`, passing
//! the subcommand name as the first argument; strip it so `cargo okf validate`
//! and a direct `cargo-okf validate` behave identically. Everything else is
//! the `okf` CLI, shared via `okf::cli`.

use std::process::ExitCode;

fn main() -> ExitCode {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("okf") {
        args.remove(0);
    }
    okf::cli::run(&args)
}
