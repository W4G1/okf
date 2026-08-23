//! The one behavior cargo-okf adds over okf: stripping the subcommand name
//! cargo inserts, so both invocation styles reach the same CLI.

use std::process::Command;

fn run(args: &[&str]) -> (String, bool) {
    let output = Command::new(env!("CARGO_BIN_EXE_cargo-okf"))
        .args(args)
        .output()
        .unwrap();
    (
        String::from_utf8(output.stdout).unwrap(),
        output.status.success(),
    )
}

#[test]
fn version_works_with_and_without_the_cargo_inserted_subcommand_name() {
    let (via_cargo, ok_cargo) = run(&["okf", "--version"]);
    let (direct, ok_direct) = run(&["--version"]);
    assert!(ok_cargo && ok_direct);
    assert_eq!(via_cargo, direct);
    assert!(via_cargo.starts_with("okf "), "got: {via_cargo}");
}
