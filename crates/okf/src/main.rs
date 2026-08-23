//! Binary entry point for the `okf` CLI. The implementation lives in the
//! library's `cli` module so the `cargo-okf` subcommand can reuse it.

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    okf::cli::run(&args)
}
