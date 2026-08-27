//! # okf-studio: an interactive terminal studio for OKF bundles
//!
//! `okf studio` is the *resident* form of the okf engine: one process that
//! holds a live, continuously re-validated model of a bundle and lets you
//! navigate, audit, and refactor it interactively.
//!
//! The crate is a library with a single entry point, [`run`]; the `okf` CLI's
//! `studio` subcommand (and `cargo okf studio`) are thin wrappers around it.
//!
//! Architecture: Elm-style unidirectional flow. The UI thread owns
//! [`app::App`] and renders from an immutable [`snapshot::Snapshot`]; a
//! worker thread performs every load and write; a polling watcher reports
//! external edits. See the workspace design document for the full picture.

#![forbid(unsafe_code)]
#![warn(missing_docs)]
#![warn(clippy::pedantic, clippy::nursery)]

pub mod app;
pub mod graph;
pub mod keymap;
pub mod markdown;
pub mod search;
pub mod snapshot;
pub mod theme;
pub mod ui;
pub mod watch;
pub mod worker;

use app::{App, Command, Msg};
use crossterm::event::{self, Event};
use okf_core::Date;
use std::io::Write as _;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::mpsc::channel;
use std::time::{Duration, Instant};

/// The four studio workspaces.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tab {
    /// Tree → document → inspector.
    #[default]
    Explorer,
    /// The link graph.
    Graph,
    /// Mission control: trust / staleness / lifecycle.
    Trust,
    /// The computations playground.
    Computations,
}

impl std::str::FromStr for Tab {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "explorer" => Ok(Self::Explorer),
            "graph" => Ok(Self::Graph),
            "trust" => Ok(Self::Trust),
            "computations" | "compute" => Ok(Self::Computations),
            other => Err(format!(
                "unknown tab {other:?} (expected explorer|graph|trust|computations)"
            )),
        }
    }
}

/// Launch options for the studio.
#[derive(Clone, Debug)]
pub struct StudioOptions {
    /// Bundle directory (defaults to `.`, same as every other subcommand).
    pub root: PathBuf,
    /// Pin "today" for deterministic staleness display (mirrors `--today`).
    pub today: Option<Date>,
    /// Disable the file watcher (single snapshot; useful over slow FS/NFS).
    pub no_watch: bool,
    /// Start on a specific workspace tab.
    pub initial_tab: Option<Tab>,
    /// Author identity for verification stamps / log entries
    /// (defaults to [`okf_core::default_author`]).
    pub author: Option<String>,
}

impl Default for StudioOptions {
    fn default() -> Self {
        Self {
            root: PathBuf::from("."),
            today: None,
            no_watch: false,
            initial_tab: None,
            author: None,
        }
    }
}

/// Runs the studio over the bundle at `options.root`. Returns the process
/// exit code.
///
/// # Errors
///
/// Returns an [`std::io::Error`] when the terminal cannot be initialized or
/// drawn to. Bundle problems are *not* errors here: they render inside the
/// studio, which is the whole point of a permissive loader.
pub fn run(options: StudioOptions) -> std::io::Result<ExitCode> {
    let (msg_tx, msg_rx) = channel::<Msg>();

    let mut app = App::new(&options);
    let _watcher = if options.no_watch {
        None
    } else {
        Some(watch::spawn(
            options.root.clone(),
            Duration::from_millis(500),
            msg_tx.clone(),
        ))
    };
    let worker_tx = worker::spawn(
        worker::WorkerConfig {
            root: options.root,
            today: options.today,
            author: app.author.clone(),
        },
        msg_tx,
    );

    // Terminal teardown is guaranteed: a panic hook restores the terminal
    // before the panic message prints (extending the CLI's hook pattern).
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        ratatui::restore();
        default_hook(info);
    }));
    let mut terminal = ratatui::try_init()?;

    let tick = Duration::from_millis(250);
    let mut last_tick = Instant::now();
    // Rendering is on-demand: draw after every message (plus the tick), not
    // on a fixed frame rate. An idle studio consumes ~0% CPU.
    let mut dirty = true;
    let result = loop {
        // Drain worker / watcher messages.
        while let Ok(msg) = msg_rx.try_recv() {
            app.update(msg);
            dirty = true;
        }
        if last_tick.elapsed() >= tick {
            last_tick = Instant::now();
            app.update(Msg::Tick);
            dirty = true;
        }

        // Dispatch requested side effects.
        for command in app.pending_commands.drain(..) {
            let _ = worker_tx.send(command);
        }
        if let Some(path) = app.editor_request.take() {
            open_in_editor(&mut terminal, &path)?;
            app.update(Msg::FilesChanged);
            dirty = true;
        }
        if let Some(text) = app.copy_request.take() {
            osc52_copy(&text);
        }
        if app.should_quit {
            break Ok(ExitCode::SUCCESS);
        }

        if dirty {
            terminal.draw(|frame| ui::shell::draw(frame, &app))?;
            dirty = false;
        }

        // Event-driven with a bounded poll so ticks and worker messages
        // still land promptly; an idle studio costs ~0% CPU.
        if event::poll(Duration::from_millis(100))? {
            match event::read()? {
                Event::Key(key) if key.is_press() => app.update(Msg::Key(key)),
                Event::Mouse(mouse) => app.update(Msg::Mouse(mouse)),
                Event::Resize(_, _) => app.update(Msg::Resize),
                _ => {}
            }
            dirty = true;
        }
    };
    let _ = worker_tx.send(Command::Shutdown);
    ratatui::restore();
    result
}

/// Suspends the TUI, runs `$EDITOR` (fallback `vi`) on `path`, and resumes.
/// This is the studio's escape hatch for free-form body edits.
fn open_in_editor(
    terminal: &mut ratatui::DefaultTerminal,
    path: &std::path::Path,
) -> std::io::Result<()> {
    let editor = std::env::var("VISUAL")
        .or_else(|_| std::env::var("EDITOR"))
        .unwrap_or_else(|_| "vi".to_string());
    ratatui::restore();
    let status = std::process::Command::new(&editor).arg(path).status();
    // Re-enter the alternate screen whatever the editor did.
    crossterm::terminal::enable_raw_mode()?;
    crossterm::execute!(std::io::stdout(), crossterm::terminal::EnterAlternateScreen)?;
    terminal.clear()?;
    if let Err(e) = status {
        // Reported after the terminal is back, so it is visible in-app.
        return Err(std::io::Error::other(format!(
            "could not launch editor {editor:?}: {e}"
        )));
    }
    Ok(())
}

/// Copies text to the system clipboard through the OSC 52 escape sequence —
/// the handoff artifact for whoever *does* execute a computation.
fn osc52_copy(text: &str) {
    let encoded = base64(text.as_bytes());
    let mut stdout = std::io::stdout();
    let _ = write!(stdout, "\x1b]52;c;{encoded}\x07");
    let _ = stdout.flush();
}

/// Minimal std-only base64 (standard alphabet, padded).
fn base64(input: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(ALPHABET[(n >> 18) as usize & 63] as char);
        out.push(ALPHABET[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}
