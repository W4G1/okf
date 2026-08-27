# okf-studio

`okf studio` — an interactive terminal IDE and mission control center for
[Open Knowledge Format (OKF)](https://github.com/W4G1/okf) bundles, built on
[Ratatui](https://ratatui.rs).

Where the `okf` CLI commands are batch verbs (`validate`, `trust`, `graph`,
`mv`, …), studio is the *resident* form of the same engine: one process that
holds a live, continuously re-validated model of a bundle and lets you
navigate, audit, and refactor it interactively — without leaving the terminal.

```text
cargo install okf     # the studio ships in the default feature set
okf studio [bundle]   # --today, --tab, --no-watch, --author
```

## Workspaces

1. **Explorer** — tree → rendered document → inspector (meta, links, sources,
   log history), with follow-able links and `$EDITOR` handoff.
2. **Graph** — the cross-link and derivation graph on a Braille canvas:
   force-directed or radial layout, egocentric focus mode, five coloring
   dimensions, fuzzy filtering.
3. **Trust** (mission control) — distribution bars, a risk-ranked attention
   queue, the merged `log.md` activity sparkline, and actor statistics.
4. **Computations** — Attested Computation contracts with health checks and an
   execution-free invocation-builder playground (OSC 52 copy).

Plus overlays available everywhere: omnisearch / command palette
(`/`, `Ctrl+P`, `>` for commands), the diagnostics & fix engine (`!`), and
contextual refactor verbs (`F2` move/rename, `Del` remove, `M` merge, split
and section rename from the outline) — every write dry-run-previewed first.

This crate is a library with a single entry point (`okf_studio::run`); the
`okf` binary's `studio` subcommand is a thin wrapper around it.
