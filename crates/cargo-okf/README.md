# cargo-okf

The [okf](https://crates.io/crates/okf) command-line tool as a cargo
subcommand for the [Open Knowledge Format
(OKF)](https://github.com/GoogleCloudPlatform/open-knowledge-format).

```sh
cargo install cargo-okf

cargo okf validate ./bundles/finance
cargo okf lint ./bundles/finance
```

Every `okf` subcommand is available; `cargo okf <cmd>` is identical to
`okf <cmd>`. See the [okf crate](https://crates.io/crates/okf) for the full
command list and library documentation.
