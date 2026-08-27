//! The application model: state, messages, commands, and the reducer.
//!
//! Elm-style unidirectional flow: `update(&mut App, Msg)` is pure state
//! mutation — it never blocks and never touches the disk. Side effects are
//! requested as [`Command`]s that the main loop drains to the worker thread,
//! plus two main-thread specials (`$EDITOR` suspension and OSC 52 copy).

use crate::graph::{LayoutEngine, LayoutMode};
use crate::keymap::{Action, Context, resolve};
use crate::snapshot::{LeafKind, Snapshot, TreeNode};
use crate::theme::Theme;
use crate::{StudioOptions, Tab};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent};
use okf_core::{ConceptId, Date, RefactorError, TrustTier};
use okf_core::{MergeReport, MoveReport, RemoveReport, RenameSectionReport, SplitReport};
use std::collections::{HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;

/// Ticks (250 ms each) a toast stays visible.
const TOAST_TTL: u8 = 12;

/// An input message consumed by [`App::update`].
#[derive(Debug)]
pub enum Msg {
    /// A key press.
    Key(KeyEvent),
    /// A mouse event.
    Mouse(MouseEvent),
    /// The terminal was resized.
    Resize,
    /// The 250 ms heartbeat: spinners, toast expiry, debounced previews,
    /// incremental layout.
    Tick,
    /// The watcher saw files change on disk.
    FilesChanged,
    /// The worker finished building a snapshot.
    SnapshotReady(Arc<Snapshot>),
    /// The worker failed to load the bundle.
    SnapshotFailed(String),
    /// A refactor dry-run finished.
    PreviewReady(u64, Result<PreviewReport, RefactorError>),
    /// A write operation finished: `Ok(toast)` or `Err(message)`.
    Applied(Result<String, String>),
    /// A bundle-wide fix dry-run finished.
    FixReportReady(Box<okf_core::BundleFixReport>),
    /// A background error worth a toast.
    Error(String),
}

/// A side-effect request executed by the worker thread.
#[derive(Clone, Debug)]
pub enum Command {
    /// Rebuild the snapshot from disk.
    Reload,
    /// Dry-run a refactor; answer with [`Msg::PreviewReady`] carrying the id.
    Preview {
        /// Correlation id for the answer.
        request: u64,
        /// The operation to simulate.
        op: RefactorOp,
    },
    /// Apply a refactor for real.
    Apply(RefactorOp),
    /// Append a verification stamp to a concept.
    StampVerification(ConceptId),
    /// Write a new `stale_after` date.
    SetStaleAfter(ConceptId, Date),
    /// Scaffold a new concept file.
    CreateConcept {
        /// Path relative to the bundle root (`.md` optional).
        rel_path: String,
        /// The concept `type`.
        type_: String,
        /// Optional explicit title.
        title: Option<String>,
    },
    /// Dry-run the bundle fix engine.
    PreviewFix,
    /// Apply all safe fixes.
    ApplyFix,
    /// Apply safe fixes to one file.
    ApplyFixFile(PathBuf),
    /// Re-pin the evaluation date (`None` returns to the wall clock).
    SetToday(Option<Date>),
    /// Stop the worker thread.
    Shutdown,
}

/// A refactor operation, shared by preview and apply.
#[derive(Clone, Debug)]
pub enum RefactorOp {
    /// Move / rename a concept.
    Move {
        /// Current id.
        source: ConceptId,
        /// New id.
        target: ConceptId,
        /// Overwrite an existing target.
        force: bool,
    },
    /// Remove a concept.
    Remove {
        /// The concept to remove.
        target: ConceptId,
        /// Redirect inbound links here.
        redirect_to: Option<ConceptId>,
        /// Unlink inbound links to plain text.
        unlink: bool,
        /// Remove even with inbound links.
        force: bool,
    },
    /// Merge `source` into `target`.
    Merge {
        /// The concept that disappears.
        source: ConceptId,
        /// The surviving concept.
        target: ConceptId,
    },
    /// Split a section out into a new concept.
    Split {
        /// The concept holding the section.
        source: ConceptId,
        /// The new concept's id.
        target: ConceptId,
        /// The section heading to extract.
        section: String,
        /// Title for the new concept.
        title: Option<String>,
        /// Overwrite an existing target.
        force: bool,
    },
    /// Rename a section heading.
    RenameSection {
        /// The concept holding the section.
        concept: ConceptId,
        /// The current heading.
        old: String,
        /// The new heading.
        new: String,
    },
}

/// A dry-run (or applied) refactor report.
#[derive(Clone, Debug)]
pub enum PreviewReport {
    /// From [`okf_core::move_concept`].
    Move(MoveReport),
    /// From [`okf_core::remove_concept`].
    Remove(RemoveReport),
    /// From [`okf_core::merge_concepts`].
    Merge(MergeReport),
    /// From [`okf_core::split_concept`].
    Split(SplitReport),
    /// From [`okf_core::rename_section`].
    RenameSection(RenameSectionReport),
}

/// What the tree selection points at. Selection is held by identifier, not
/// index, so it survives snapshot swaps.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TreeSel {
    /// A directory (by `/`-joined relative path).
    Dir(String),
    /// A concept.
    Concept(ConceptId),
    /// A reserved or broken file.
    File(PathBuf),
}

/// One visible row of the flattened tree.
#[derive(Clone, Debug)]
pub struct TreeRow {
    /// Indentation depth.
    pub depth: usize,
    /// The selection identifier.
    pub sel: TreeSel,
    /// Display name.
    pub name: String,
    /// Row payload.
    pub kind: TreeRowKind,
}

/// The payload of a tree row.
#[derive(Clone, Debug)]
pub enum TreeRowKind {
    /// A directory row.
    Dir {
        /// Recursive concept count.
        count: usize,
        /// Whether it is currently collapsed.
        collapsed: bool,
    },
    /// A file row.
    Leaf(LeafKind),
}

/// Which explorer pane has focus.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ExplorerPane {
    /// The tree.
    #[default]
    Tree,
    /// The document viewer.
    Viewer,
}

/// Explorer workspace state.
#[derive(Debug, Default)]
pub struct ExplorerState {
    /// Focused pane.
    pub pane: ExplorerPane,
    /// Current selection.
    pub selected: Option<TreeSel>,
    /// Collapsed directory paths.
    pub collapsed: HashSet<String>,
    /// Tree scroll offset, written back by the view.
    pub tree_offset: std::cell::Cell<usize>,
    /// Viewer scroll offset (rendered lines).
    pub scroll: usize,
    /// The height-clamped maximum scroll, written back by the view.
    pub max_scroll: std::cell::Cell<usize>,
    /// Focused link index in the rendered document.
    pub focused_link: Option<usize>,
    /// Whether frontmatter shows as raw YAML.
    pub raw_yaml: bool,
    /// Inspector tab: 0 Meta, 1 Links, 2 Sources, 3 History.
    pub inspector_tab: usize,
    /// Back stack of viewed concepts.
    pub history: Vec<ConceptId>,
    /// Forward stack.
    pub future: Vec<ConceptId>,
}

/// The dimension the graph colors nodes by.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ColorBy {
    /// Trust tier (the default and the studio's primary hue).
    #[default]
    Trust,
    /// Lifecycle status.
    Status,
    /// Staleness.
    Staleness,
    /// Concept type.
    Type,
    /// Open diagnostics.
    Diagnostics,
}

impl ColorBy {
    /// The next dimension in the cycle.
    #[must_use]
    pub const fn next(self) -> Self {
        match self {
            Self::Trust => Self::Status,
            Self::Status => Self::Staleness,
            Self::Staleness => Self::Type,
            Self::Type => Self::Diagnostics,
            Self::Diagnostics => Self::Trust,
        }
    }

    /// Display name.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Trust => "trust",
            Self::Status => "status",
            Self::Staleness => "staleness",
            Self::Type => "type",
            Self::Diagnostics => "diagnostics",
        }
    }
}

/// Graph workspace state.
#[derive(Debug)]
pub struct GraphState {
    /// The layout engine (positions persist across snapshots).
    pub layout: LayoutEngine,
    /// The layout algorithm.
    pub mode: LayoutMode,
    /// Pan offset in layout space.
    pub pan: (f64, f64),
    /// Zoom factor (larger = closer).
    pub zoom: f64,
    /// Selected node key.
    pub selected: Option<String>,
    /// Egocentric focus: center node key and hop count (cycles 1→2→3→off).
    pub focus: Option<(String, usize)>,
    /// Coloring dimension.
    pub color_by: ColorBy,
    /// Whether external source nodes show.
    pub show_sources: bool,
    /// Whether broken-target nodes show.
    pub show_broken: bool,
    /// Whether derivation edges show.
    pub show_derivations: bool,
    /// The applied fuzzy node filter.
    pub filter: String,
    /// The filter input line, when open.
    pub filter_input: Option<String>,
}

impl Default for GraphState {
    fn default() -> Self {
        Self {
            layout: LayoutEngine::default(),
            mode: LayoutMode::default(),
            pan: (0.0, 0.0),
            zoom: 1.0,
            selected: None,
            focus: None,
            color_by: ColorBy::default(),
            show_sources: false,
            show_broken: true,
            show_derivations: true,
            filter: String::new(),
            filter_input: None,
        }
    }
}

/// Which mission-control panel has focus.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TrustPanel {
    /// The attention queue.
    #[default]
    Queue,
    /// The trust-tier distribution bars.
    TrustBars,
    /// The lifecycle distribution bars.
    LifecycleBars,
    /// The freshness distribution bars.
    FreshnessBars,
    /// The activity sparkline.
    Activity,
}

impl TrustPanel {
    /// The next panel in the focus cycle.
    #[must_use]
    pub const fn next(self) -> Self {
        match self {
            Self::Queue => Self::TrustBars,
            Self::TrustBars => Self::LifecycleBars,
            Self::LifecycleBars => Self::FreshnessBars,
            Self::FreshnessBars => Self::Activity,
            Self::Activity => Self::Queue,
        }
    }
}

/// A cohort filter applied to the attention queue.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cohort {
    /// Filter to a trust tier.
    Tier(TrustTier),
    /// Filter to a status bucket (index into `status_counts`).
    Status(usize),
    /// Freshness bucket: 0 fresh, 1 stale, 2 stale-soon.
    Fresh(usize),
}

/// Attention-queue sort order.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum QueueSort {
    /// By risk score (default).
    #[default]
    Risk,
    /// By days overdue.
    Staleness,
    /// By blast radius (backlinks).
    Backlinks,
}

impl QueueSort {
    /// The next sort in the cycle.
    #[must_use]
    pub const fn next(self) -> Self {
        match self {
            Self::Risk => Self::Staleness,
            Self::Staleness => Self::Backlinks,
            Self::Backlinks => Self::Risk,
        }
    }

    /// Display name.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Risk => "risk",
            Self::Staleness => "staleness",
            Self::Backlinks => "backlinks",
        }
    }
}

/// Mission-control workspace state.
#[derive(Debug, Default)]
pub struct TrustState {
    /// Focused panel.
    pub panel: TrustPanel,
    /// Selected queue row.
    pub queue_sel: usize,
    /// Selected bar row within the focused bar panel.
    pub bar_sel: usize,
    /// Active cohort filter.
    pub cohort: Option<Cohort>,
    /// Queue sort.
    pub sort: QueueSort,
}

/// Which computations pane has focus.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CompPane {
    /// The contract list.
    #[default]
    List,
    /// The playground form.
    Form,
}

/// Computations workspace state.
#[derive(Debug, Default)]
pub struct ComputationsState {
    /// Focused pane.
    pub pane: CompPane,
    /// Selected contract index.
    pub selected: usize,
    /// Focused form field.
    pub field: usize,
    /// Parameter values, keyed by `concept-id\u{0}param-name`.
    pub values: std::collections::HashMap<String, String>,
}

/// The palette's mode, derived from its input prefix.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaletteMode {
    /// Omnisearch over concepts.
    Search,
    /// Command mode (`>` prefix).
    Command,
}

/// Palette overlay state.
#[derive(Debug, Default)]
pub struct PaletteState {
    /// The typed query.
    pub input: String,
    /// Selected result row.
    pub sel: usize,
}

impl PaletteState {
    /// The active mode.
    #[must_use]
    pub fn mode(&self) -> PaletteMode {
        if self.input.trim_start().starts_with('>') {
            PaletteMode::Command
        } else {
            PaletteMode::Search
        }
    }
}

/// Diagnostics overlay filter.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DiagFilter {
    /// Everything.
    #[default]
    All,
    /// Validation errors only.
    Errors,
    /// Validation warnings only.
    Warnings,
    /// Lint findings only.
    Lint,
}

/// Diagnostics overlay state.
#[derive(Debug, Default)]
pub struct DiagnosticsState {
    /// Active filter.
    pub filter: DiagFilter,
    /// Selected row.
    pub sel: usize,
}

/// What a confirm overlay confirms.
#[derive(Clone, Debug)]
pub enum ConfirmAction {
    /// Stamp a verification.
    Verify(ConceptId),
}

/// Confirm overlay state.
#[derive(Debug)]
pub struct ConfirmState {
    /// Title line.
    pub title: String,
    /// Body lines.
    pub body: Vec<String>,
    /// What Enter does.
    pub action: ConfirmAction,
}

/// Date-picker overlay state (stale extension / today pinning).
#[derive(Debug)]
pub struct DatePickerState {
    /// The concept whose `stale_after` is written; `None` pins `--today`.
    pub id: Option<ConceptId>,
    /// The typed date.
    pub input: String,
}

/// The refactor verb a modal drives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VerbKind {
    /// Move / rename.
    Move,
    /// Remove.
    Remove,
    /// Merge into another concept.
    Merge,
    /// Split a section out.
    Split,
    /// Rename a section heading.
    RenameSection,
}

/// The remove modal's decision, mapped 1:1 to [`okf_core::RemoveOptions`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RemoveChoice {
    /// No inbound links (or not decided yet).
    #[default]
    Plain,
    /// Redirect inbound links.
    Redirect,
    /// Unlink inbound links to plain text.
    Unlink,
    /// Force: leave broken links.
    Force,
}

/// Refactor modal state: intent → (decision) → preview.
#[derive(Debug)]
pub struct RefactorState {
    /// The verb.
    pub verb: VerbKind,
    /// The subject (studio already knows what you're looking at).
    pub subject: ConceptId,
    /// The section, for the heading verbs.
    pub section: Option<String>,
    /// Main input: target id or new heading text.
    pub input: String,
    /// Secondary input: split title / remove redirect target.
    pub extra: String,
    /// Which input is active (0 main, 1 extra).
    pub field: usize,
    /// The remove decision, when the dry-run surfaced inbound links.
    pub choice: RemoveChoice,
    /// Whether the move target may overwrite (set after the engine asks).
    pub force: bool,
    /// The latest dry-run result.
    pub preview: Option<Result<PreviewReport, RefactorError>>,
    /// The latest request id sent.
    pub request: u64,
    /// Debounce flag: a preview should be sent on the next tick.
    pub needs_preview: bool,
}

/// New-concept form state.
#[derive(Debug)]
pub struct NewConceptState {
    /// Field values: path, type, title.
    pub fields: [String; 3],
    /// Focused field.
    pub field: usize,
}

/// Fix preview overlay state.
#[derive(Debug)]
pub struct FixPreviewState {
    /// The dry-run report (contents carry before/after text).
    pub report: Box<okf_core::BundleFixReport>,
    /// Selected changed file.
    pub file_sel: usize,
    /// Diff scroll.
    pub scroll: usize,
}

/// Outline overlay state.
#[derive(Debug)]
pub struct OutlineState {
    /// The concept whose outline shows.
    pub id: ConceptId,
    /// Selected heading row.
    pub sel: usize,
    /// When set, the selected heading is being renamed with this input.
    pub rename: Option<String>,
}

/// An open overlay. Overlays stack; `Esc` closes the topmost.
#[derive(Debug)]
pub enum Overlay {
    /// Omnisearch / command palette.
    Palette(PaletteState),
    /// Diagnostics & fix engine.
    Diagnostics(DiagnosticsState),
    /// Help cheatsheet (scroll offset).
    Help(usize),
    /// Confirmation prompt.
    Confirm(ConfirmState),
    /// Date picker.
    DatePicker(DatePickerState),
    /// Refactor modal.
    Refactor(Box<RefactorState>),
    /// New-concept form.
    NewConcept(NewConceptState),
    /// Fix preview diff.
    FixPreview(FixPreviewState),
    /// Full merged log view (scroll offset).
    LogView(usize),
    /// Outline jump list.
    Outline(OutlineState),
}

/// A transient status message.
#[derive(Clone, Debug)]
pub struct Toast {
    /// The text.
    pub text: String,
    /// Remaining ticks.
    pub ttl: u8,
    /// Whether it reports an error.
    pub error: bool,
}

/// What background work is in flight, for the status-bar spinner.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LoadPhase {
    /// Nothing in flight.
    #[default]
    Idle,
    /// A snapshot rebuild is in flight.
    Reloading,
    /// A write operation is in flight.
    Applying,
}

/// The whole application state.
#[allow(clippy::struct_excessive_bools)]
pub struct App {
    /// The immutable world model (absent until the first load lands).
    pub snapshot: Option<Arc<Snapshot>>,
    /// The active workspace.
    pub tab: Tab,
    /// Explorer state.
    pub explorer: ExplorerState,
    /// Graph state.
    pub graph: GraphState,
    /// Mission-control state.
    pub trust: TrustState,
    /// Computations state.
    pub computations: ComputationsState,
    /// The overlay stack.
    pub overlays: Vec<Overlay>,
    /// Transient status messages.
    pub toasts: VecDeque<Toast>,
    /// Background work indicator.
    pub loading: LoadPhase,
    /// The theme.
    pub theme: Theme,
    /// Author identity for stamps and log entries.
    pub author: String,
    /// Commands awaiting dispatch to the worker (drained by the main loop).
    pub pending_commands: Vec<Command>,
    /// A file to open in `$EDITOR` (handled by the main loop).
    pub editor_request: Option<PathBuf>,
    /// Text to copy to the clipboard via OSC 52 (handled by the main loop).
    pub copy_request: Option<String>,
    /// Set when the user quits.
    pub should_quit: bool,
    /// Tick counter.
    pub tick: u64,
    /// A reload arrived while one was in flight.
    pub reload_queued: bool,
    /// Monotonic preview-request counter.
    pub next_request: u64,
    /// The bundle directory's display name.
    pub root_name: String,
    /// A fatal load error, shown instead of a workspace.
    pub load_error: Option<String>,
    /// Whether the watcher is disabled.
    pub no_watch: bool,
    /// The first snapshot has not yet been inspected (focus-mode default).
    first_snapshot: bool,
}

impl App {
    /// Creates the initial state from the launch options.
    #[must_use]
    pub fn new(options: &StudioOptions) -> Self {
        let root_name = options
            .root
            .canonicalize()
            .unwrap_or_else(|_| options.root.clone())
            .file_name()
            .map_or_else(
                || options.root.display().to_string(),
                |n| n.to_string_lossy().into_owned(),
            );
        Self {
            snapshot: None,
            tab: options.initial_tab.unwrap_or(Tab::Explorer),
            explorer: ExplorerState::default(),
            graph: GraphState::default(),
            trust: TrustState::default(),
            computations: ComputationsState::default(),
            overlays: Vec::new(),
            toasts: VecDeque::new(),
            loading: LoadPhase::Reloading,
            theme: Theme::from_env(),
            author: options
                .author
                .clone()
                .unwrap_or_else(okf_core::default_author),
            pending_commands: vec![Command::Reload],
            editor_request: None,
            copy_request: None,
            should_quit: false,
            tick: 0,
            reload_queued: false,
            next_request: 0,
            root_name,
            load_error: None,
            no_watch: options.no_watch,
            first_snapshot: true,
        }
    }

    /// Pushes a toast.
    pub fn toast(&mut self, text: impl Into<String>, error: bool) {
        self.toasts.push_back(Toast {
            text: text.into(),
            ttl: TOAST_TTL,
            error,
        });
        while self.toasts.len() > 3 {
            self.toasts.pop_front();
        }
    }

    fn send(&mut self, command: Command) {
        self.pending_commands.push(command);
    }

    /// The concept the current selection denotes, if any — the subject every
    /// contextual verb acts on.
    #[must_use]
    pub fn selected_concept(&self) -> Option<ConceptId> {
        let snapshot = self.snapshot.as_ref()?;
        match self.tab {
            Tab::Explorer => match &self.explorer.selected {
                Some(TreeSel::Concept(id)) => Some(id.clone()),
                _ => None,
            },
            Tab::Graph => {
                let key = self.graph.selected.as_ref()?;
                snapshot
                    .graph
                    .nodes
                    .iter()
                    .find(|n| &n.key == key)
                    .and_then(|n| n.id.clone())
            }
            Tab::Trust => self
                .filtered_queue()
                .get(self.trust.queue_sel)
                .map(|item| item.id.clone()),
            Tab::Computations => snapshot
                .contracts
                .get(self.computations.selected)
                .map(|c| c.id.clone()),
        }
    }

    /// The on-disk file the current selection denotes (for `$EDITOR`).
    #[must_use]
    pub fn selected_path(&self) -> Option<PathBuf> {
        let snapshot = self.snapshot.as_ref()?;
        if self.tab == Tab::Explorer {
            match &self.explorer.selected {
                Some(TreeSel::File(path)) => return Some(path.clone()),
                Some(TreeSel::Dir(_)) | None => return None,
                Some(TreeSel::Concept(_)) => {}
            }
        }
        self.selected_concept()
            .map(|id| id.to_path(snapshot.bundle.root()))
    }

    /// The visible tree rows given the current collapse state.
    #[must_use]
    pub fn tree_rows(&self) -> Vec<TreeRow> {
        let mut rows = Vec::new();
        if let Some(snapshot) = &self.snapshot {
            flatten_tree(&snapshot.tree.roots, 0, &self.explorer.collapsed, &mut rows);
        }
        rows
    }

    /// The attention queue after the cohort filter and sort.
    #[must_use]
    pub fn filtered_queue(&self) -> Vec<crate::snapshot::AttentionItem> {
        let Some(snapshot) = &self.snapshot else {
            return Vec::new();
        };
        let mut items: Vec<crate::snapshot::AttentionItem> = snapshot
            .attention
            .iter()
            .filter(|item| {
                let Some(meta) = snapshot.meta(&item.id) else {
                    return false;
                };
                match self.trust.cohort {
                    None => true,
                    Some(Cohort::Tier(tier)) => meta.tier == tier,
                    Some(Cohort::Status(ix)) => {
                        ix == match meta.status {
                            okf_core::Status::Draft => 0,
                            okf_core::Status::Stable => 1,
                            okf_core::Status::Deprecated => 2,
                            okf_core::Status::Other(_) => 3,
                        }
                    }
                    Some(Cohort::Fresh(ix)) => match ix {
                        1 => meta.stale,
                        2 => meta.stale_in_days.is_some(),
                        _ => !meta.stale && meta.stale_in_days.is_none(),
                    },
                }
            })
            .cloned()
            .collect();
        match self.trust.sort {
            QueueSort::Risk => {}
            QueueSort::Staleness => items.sort_by_key(|item| {
                std::cmp::Reverse(
                    self.snapshot
                        .as_ref()
                        .and_then(|s| s.meta(&item.id))
                        .and_then(|m| m.overdue_days)
                        .unwrap_or(i64::MIN),
                )
            }),
            QueueSort::Backlinks => items.sort_by_key(|item| {
                std::cmp::Reverse(
                    self.snapshot
                        .as_ref()
                        .and_then(|s| s.meta(&item.id))
                        .map_or(0, |m| m.in_degree),
                )
            }),
        }
        items
    }

    /// Applies one message. Never blocks, never touches disk.
    pub fn update(&mut self, msg: Msg) {
        match msg {
            Msg::Key(key) => self.on_key(key),
            Msg::Mouse(_) | Msg::Resize => {}
            Msg::Tick => self.on_tick(),
            Msg::FilesChanged => {
                if self.loading == LoadPhase::Idle {
                    self.loading = LoadPhase::Reloading;
                    self.send(Command::Reload);
                } else {
                    self.reload_queued = true;
                }
            }
            Msg::SnapshotReady(snapshot) => self.on_snapshot(snapshot),
            Msg::SnapshotFailed(e) => {
                self.load_error = Some(e);
                self.loading = LoadPhase::Idle;
            }
            Msg::PreviewReady(request, result) => {
                if let Some(Overlay::Refactor(state)) = self.overlays.last_mut()
                    && state.request == request
                {
                    if let Err(RefactorError::HasInboundLinks { .. }) = &result
                        && state.verb == VerbKind::Remove
                        && state.choice == RemoveChoice::Plain
                    {
                        // The decision stage: the modal now asks exactly the
                        // question the engine raised.
                    }
                    state.preview = Some(result);
                }
            }
            Msg::Applied(result) => {
                self.loading = LoadPhase::Idle;
                match result {
                    Ok(text) => self.toast(text, false),
                    Err(e) => self.toast(format!("✗ {e}"), true),
                }
            }
            Msg::FixReportReady(report) => {
                if report.is_empty() {
                    self.toast("✔ nothing to fix", false);
                } else {
                    self.overlays.push(Overlay::FixPreview(FixPreviewState {
                        report,
                        file_sel: 0,
                        scroll: 0,
                    }));
                }
            }
            Msg::Error(e) => self.toast(format!("✗ {e}"), true),
        }
    }

    fn on_tick(&mut self) {
        self.tick += 1;
        for toast in &mut self.toasts {
            toast.ttl = toast.ttl.saturating_sub(1);
        }
        self.toasts.retain(|t| t.ttl > 0);

        // Debounced refactor preview.
        let mut to_send: Option<(u64, RefactorOp)> = None;
        if let Some(Overlay::Refactor(state)) = self.overlays.last_mut()
            && state.needs_preview
        {
            state.needs_preview = false;
            if let Some(op) = build_op(state) {
                self.next_request += 1;
                state.request = self.next_request;
                to_send = Some((state.request, op));
            } else {
                state.preview = None;
            }
        }
        if let Some((request, op)) = to_send {
            self.send(Command::Preview { request, op });
        }

        // Incremental layout stepping while the graph is visible.
        if self.tab == Tab::Graph
            && self.graph.mode == LayoutMode::Force
            && let Some(snapshot) = self.snapshot.clone()
        {
            let included = self.graph_included(&snapshot);
            self.graph.layout.step(&snapshot.graph, &included, 12);
        }
    }

    /// The nodes currently visible in the graph, honoring toggles, focus
    /// mode, and the fuzzy filter (filtered nodes stay visible but dimmed —
    /// they remain in layout).
    #[must_use]
    pub fn graph_included(&self, snapshot: &Snapshot) -> Vec<bool> {
        let model = &snapshot.graph;
        let mut included: Vec<bool> = model
            .nodes
            .iter()
            .map(|node| match node.kind {
                crate::graph::NodeKind::Source => self.graph.show_sources,
                crate::graph::NodeKind::Phantom => self.graph.show_broken,
                _ => true,
            })
            .collect();
        if let Some((center_key, k)) = &self.graph.focus
            && let Some(center) = model.nodes.iter().position(|n| &n.key == center_key)
        {
            let hood = model.neighborhood(center, *k);
            for (i, inc) in included.iter_mut().enumerate() {
                *inc = *inc && hood[i];
            }
        }
        included
    }

    fn on_snapshot(&mut self, snapshot: Arc<Snapshot>) {
        self.load_error = None;
        self.loading = LoadPhase::Idle;

        // Selection survival: fall back when the selected thing vanished.
        if let Some(TreeSel::Concept(id)) = &self.explorer.selected
            && !snapshot.bundle.contains(id)
        {
            self.explorer.selected = None;
        }
        if self.explorer.selected.is_none() {
            self.explorer.selected = snapshot
                .bundle
                .concepts()
                .first()
                .map(|c| TreeSel::Concept(c.id.clone()));
        }
        if let Some(key) = &self.graph.selected
            && !snapshot.graph.nodes.iter().any(|n| &n.key == key)
        {
            self.graph.selected = None;
        }
        self.computations.selected = self
            .computations
            .selected
            .min(snapshot.contracts.len().saturating_sub(1));

        // Layout: carry positions over; seed new nodes; keep radial layouts.
        self.graph.layout.seed(&snapshot.graph);
        if self.graph.mode == LayoutMode::Radial {
            self.graph.layout.radial(&snapshot.graph);
        }

        // Egocentric default for large bundles: the full hairball is not a
        // useful first picture past ~500 nodes.
        if self.first_snapshot {
            self.first_snapshot = false;
            if snapshot.graph.nodes.len() > 500 {
                let center = snapshot.bundle.concepts().first().map(|c| c.id.to_string());
                if let Some(center) = center {
                    self.graph.focus = Some((center, 1));
                }
            }
        }

        self.snapshot = Some(snapshot);
        if self.reload_queued {
            self.reload_queued = false;
            self.loading = LoadPhase::Reloading;
            self.send(Command::Reload);
        }
    }
}

fn flatten_tree(
    nodes: &[TreeNode],
    depth: usize,
    collapsed: &HashSet<String>,
    out: &mut Vec<TreeRow>,
) {
    for node in nodes {
        match node {
            TreeNode::Dir {
                name,
                path,
                concept_count,
                children,
            } => {
                let is_collapsed = collapsed.contains(path);
                out.push(TreeRow {
                    depth,
                    sel: TreeSel::Dir(path.clone()),
                    name: name.clone(),
                    kind: TreeRowKind::Dir {
                        count: *concept_count,
                        collapsed: is_collapsed,
                    },
                });
                if !is_collapsed {
                    flatten_tree(children, depth + 1, collapsed, out);
                }
            }
            TreeNode::Leaf { name, kind } => {
                let sel = match kind {
                    LeafKind::Concept(id) => TreeSel::Concept(id.clone()),
                    LeafKind::Index(p) | LeafKind::Log(p) | LeafKind::Broken(p) => {
                        TreeSel::File(p.clone())
                    }
                };
                out.push(TreeRow {
                    depth,
                    sel,
                    name: name.clone(),
                    kind: TreeRowKind::Leaf(kind.clone()),
                });
            }
        }
    }
}

/// Builds the refactor operation a modal currently describes, or `None` when
/// its inputs are not yet valid.
fn build_op(state: &RefactorState) -> Option<RefactorOp> {
    match state.verb {
        VerbKind::Move => {
            let target = ConceptId::parse(state.input.trim()).ok()?;
            Some(RefactorOp::Move {
                source: state.subject.clone(),
                target,
                force: state.force,
            })
        }
        VerbKind::Remove => {
            let redirect_to = if state.choice == RemoveChoice::Redirect {
                Some(ConceptId::parse(state.extra.trim()).ok()?)
            } else {
                None
            };
            Some(RefactorOp::Remove {
                target: state.subject.clone(),
                redirect_to,
                unlink: state.choice == RemoveChoice::Unlink,
                force: state.choice == RemoveChoice::Force,
            })
        }
        VerbKind::Merge => {
            let target = ConceptId::parse(state.input.trim()).ok()?;
            if target == state.subject {
                return None;
            }
            Some(RefactorOp::Merge {
                source: state.subject.clone(),
                target,
            })
        }
        VerbKind::Split => {
            let target = ConceptId::parse(state.input.trim()).ok()?;
            let title = if state.extra.trim().is_empty() {
                None
            } else {
                Some(state.extra.trim().to_string())
            };
            Some(RefactorOp::Split {
                source: state.subject.clone(),
                target,
                section: state.section.clone()?,
                title,
                force: state.force,
            })
        }
        VerbKind::RenameSection => {
            let new = state.input.trim();
            if new.is_empty() {
                return None;
            }
            Some(RefactorOp::RenameSection {
                concept: state.subject.clone(),
                old: state.section.clone()?,
                new: new.to_string(),
            })
        }
    }
}

// ---------------------------------------------------------------------------
// Key dispatch.
// ---------------------------------------------------------------------------

impl App {
    fn on_key(&mut self, key: KeyEvent) {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.should_quit = true;
            return;
        }
        if self.overlays.is_empty() {
            // Graph filter input captures keys before the keymap.
            if self.tab == Tab::Graph && self.graph.filter_input.is_some() {
                self.graph_filter_key(key);
                return;
            }
            // Playground form typing captures printable keys.
            if self.tab == Tab::Computations
                && self.computations.pane == CompPane::Form
                && self.playground_form_key(&key)
            {
                return;
            }
            let mut contexts: Vec<Context> = vec![match (self.tab, self.explorer.pane) {
                (Tab::Explorer, ExplorerPane::Tree) => Context::Tree,
                (Tab::Explorer, ExplorerPane::Viewer) => Context::Viewer,
                (Tab::Graph, _) => Context::Graph,
                (Tab::Trust, _) => Context::Trust,
                (Tab::Computations, _) => Context::Computations,
            }];
            if self.selected_concept().is_some() {
                contexts.push(Context::Concept);
            }
            contexts.push(Context::Global);
            if let Some(action) = resolve(&contexts, &key) {
                self.run_action(action);
            }
            return;
        }
        self.overlay_key(key);
    }

    /// Executes one keymap action (also the palette command executor).
    #[allow(clippy::too_many_lines)]
    pub fn run_action(&mut self, action: Action) {
        match action {
            Action::SwitchTab(ix) => {
                self.tab = match ix {
                    1 => Tab::Graph,
                    2 => Tab::Trust,
                    3 => Tab::Computations,
                    _ => Tab::Explorer,
                };
            }
            Action::OpenPalette => self
                .overlays
                .push(Overlay::Palette(PaletteState::default())),
            Action::OpenDiagnostics => self
                .overlays
                .push(Overlay::Diagnostics(DiagnosticsState::default())),
            Action::OpenHelp => self.overlays.push(Overlay::Help(0)),
            Action::Reload => {
                self.loading = LoadPhase::Reloading;
                self.send(Command::Reload);
            }
            Action::OpenEditor => {
                if let Some(path) = self.selected_path() {
                    self.editor_request = Some(path);
                } else {
                    self.toast("no file selected", true);
                }
            }
            Action::Quit => self.should_quit = true,
            Action::Verify => {
                if let Some(id) = self.selected_concept() {
                    self.overlays.push(Overlay::Confirm(ConfirmState {
                        title: format!("Stamp verification on {id}"),
                        body: vec![
                            format!("by {}", self.author),
                            "Appends a { by, at } event to `verified` and logs it.".to_string(),
                        ],
                        action: ConfirmAction::Verify(id),
                    }));
                }
            }
            Action::ExtendStale => {
                if let Some(id) = self.selected_concept() {
                    let current = self
                        .snapshot
                        .as_ref()
                        .and_then(|s| s.meta(&id))
                        .and_then(|m| m.stale_after.as_ref())
                        .and_then(|f| f.datetime.map(|dt| dt.date));
                    let today = self.snapshot.as_ref().map_or(
                        Date {
                            year: 2026,
                            month: 1,
                            day: 1,
                        },
                        |s| s.today,
                    );
                    let default = Date::from_days_since_epoch(
                        current
                            .unwrap_or(today)
                            .days_since_epoch()
                            .max(today.days_since_epoch())
                            + 90,
                    );
                    self.overlays.push(Overlay::DatePicker(DatePickerState {
                        id: Some(id),
                        input: default.to_string(),
                    }));
                }
            }
            Action::MoveOrRename => {
                if let Some(id) = self.selected_concept() {
                    self.open_refactor(VerbKind::Move, id, None);
                }
            }
            Action::Remove => {
                if let Some(id) = self.selected_concept() {
                    self.open_refactor(VerbKind::Remove, id, None);
                }
            }
            Action::Merge => {
                if let Some(id) = self.selected_concept() {
                    self.open_refactor(VerbKind::Merge, id, None);
                }
            }
            Action::SplitSection | Action::Outline => self.open_outline(),
            Action::NewConcept => {
                let dir = match &self.explorer.selected {
                    Some(TreeSel::Dir(path)) => path.clone(),
                    Some(TreeSel::Concept(id)) => {
                        id.parent().map(|p| p.to_string()).unwrap_or_default()
                    }
                    _ => String::new(),
                };
                let path = if dir.is_empty() {
                    String::new()
                } else {
                    format!("{dir}/")
                };
                self.overlays.push(Overlay::NewConcept(NewConceptState {
                    fields: [path, "Concept".to_string(), String::new()],
                    field: 0,
                }));
            }
            Action::CycleInspector => {
                self.explorer.inspector_tab = (self.explorer.inspector_tab + 1) % 4;
            }
            Action::ToggleRawYaml => self.explorer.raw_yaml = !self.explorer.raw_yaml,
            Action::NextLink | Action::PrevLink => self.cycle_link(action == Action::NextLink),
            Action::Back => self.nav_back(),
            Action::Forward => self.nav_forward(),
            Action::ZoomIn => self.graph.zoom = (self.graph.zoom * 1.25).min(40.0),
            Action::ZoomOut => self.graph.zoom = (self.graph.zoom / 1.25).max(0.05),
            Action::NextNode | Action::PrevNode => self.cycle_node(action == Action::NextNode),
            Action::FocusMode => self.cycle_focus_mode(),
            Action::CycleColor => self.graph.color_by = self.graph.color_by.next(),
            Action::ToggleSources => self.graph.show_sources = !self.graph.show_sources,
            Action::ToggleBroken => self.graph.show_broken = !self.graph.show_broken,
            Action::ToggleDerivations => {
                self.graph.show_derivations = !self.graph.show_derivations;
            }
            Action::PauseLayout => self.graph.layout.toggle_running(),
            Action::GraphFilter => {
                self.graph.filter_input = Some(self.graph.filter.clone());
            }
            Action::CycleLayout => {
                self.graph.mode = self.graph.mode.next();
                if let Some(snapshot) = &self.snapshot {
                    match self.graph.mode {
                        LayoutMode::Radial => self.graph.layout.radial(&snapshot.graph),
                        LayoutMode::Force => {
                            self.graph.layout.running = true;
                            self.graph.layout.toggle_running();
                            self.graph.layout.toggle_running();
                        }
                    }
                }
            }
            Action::CyclePanel => match self.tab {
                Tab::Trust => {
                    self.trust.panel = self.trust.panel.next();
                    self.trust.bar_sel = 0;
                }
                Tab::Computations => {
                    self.computations.pane = match self.computations.pane {
                        CompPane::List => CompPane::Form,
                        CompPane::Form => CompPane::List,
                    };
                }
                _ => {}
            },
            Action::CycleSort => self.trust.sort = self.trust.sort.next(),
            Action::CopySketch => self.copy_sketch(),
            Action::FixAll => self.send(Command::PreviewFix),
            Action::PinToday => {
                let today = self
                    .snapshot
                    .as_ref()
                    .map_or_else(String::new, |s| s.today.to_string());
                self.overlays.push(Overlay::DatePicker(DatePickerState {
                    id: None,
                    input: today,
                }));
            }
            Action::OpenLog => self.overlays.push(Overlay::LogView(0)),
            Action::Up
            | Action::Down
            | Action::Left
            | Action::Right
            | Action::PageUp
            | Action::PageDown
            | Action::HalfUp
            | Action::HalfDown
            | Action::Home
            | Action::End
            | Action::Activate => self.navigate(action),
        }
    }

    fn open_refactor(&mut self, verb: VerbKind, subject: ConceptId, section: Option<String>) {
        let input = match verb {
            VerbKind::Move => subject.to_string(),
            VerbKind::RenameSection => section.clone().unwrap_or_default(),
            VerbKind::Split => section
                .as_deref()
                .map(|s| {
                    let slug = okf_core::heading_slug(s);
                    subject
                        .parent()
                        .map_or_else(|| slug.clone(), |p| format!("{p}/{slug}"))
                })
                .unwrap_or_default(),
            _ => String::new(),
        };
        let extra = match verb {
            VerbKind::Split => section.clone().unwrap_or_default(),
            _ => String::new(),
        };
        let mut state = RefactorState {
            verb,
            subject,
            section,
            input,
            extra,
            field: 0,
            choice: RemoveChoice::Plain,
            force: false,
            preview: None,
            request: 0,
            needs_preview: true,
        };
        // Remove needs no input: dry-run immediately.
        if verb == VerbKind::Remove {
            state.needs_preview = true;
        }
        self.overlays.push(Overlay::Refactor(Box::new(state)));
    }

    fn open_outline(&mut self) {
        if self.tab != Tab::Explorer {
            return;
        }
        if let Some(TreeSel::Concept(id)) = &self.explorer.selected {
            self.overlays.push(Overlay::Outline(OutlineState {
                id: id.clone(),
                sel: 0,
                rename: None,
            }));
        }
    }

    fn copy_sketch(&mut self) {
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        let Some(info) = snapshot.contracts.get(self.computations.selected) else {
            return;
        };
        let mut args: Vec<String> = Vec::new();
        for parameter in &info.contract.parameters {
            let name = parameter.name.clone().unwrap_or_default();
            let key = format!("{}\u{0}{}", info.id, name);
            let value = self
                .computations
                .values
                .get(&key)
                .cloned()
                .unwrap_or_default();
            if !value.is_empty() || parameter.is_required() {
                args.push(format!("{name}={value}"));
            }
        }
        let sketch = format!("{}({})", info.id.name(), args.join(", "));
        self.copy_request = Some(sketch.clone());
        self.toast(format!("✔ copied: {sketch}"), false);
    }

    fn graph_filter_key(&mut self, key: KeyEvent) {
        let Some(input) = self.graph.filter_input.as_mut() else {
            return;
        };
        match key.code {
            KeyCode::Esc => {
                self.graph.filter_input = None;
                self.graph.filter.clear();
            }
            KeyCode::Enter => {
                self.graph.filter = self.graph.filter_input.take().unwrap_or_default();
            }
            KeyCode::Backspace => {
                input.pop();
            }
            KeyCode::Char(c) => input.push(c),
            _ => {}
        }
    }

    /// Handles printable input for the playground form. Returns `true` when
    /// the key was consumed.
    fn playground_form_key(&mut self, key: &KeyEvent) -> bool {
        let Some(snapshot) = &self.snapshot else {
            return false;
        };
        let Some(info) = snapshot.contracts.get(self.computations.selected) else {
            return false;
        };
        let params: Vec<String> = info
            .contract
            .parameters
            .iter()
            .map(|p| p.name.clone().unwrap_or_default())
            .collect();
        if params.is_empty() {
            return false;
        }
        let field = self.computations.field.min(params.len() - 1);
        let value_key = format!("{}\u{0}{}", info.id, params[field]);
        match key.code {
            KeyCode::Char(c)
                if !key.modifiers.contains(KeyModifiers::CONTROL)
                    && c != '?'
                    && c != '!'
                    && c != '/' =>
            {
                self.computations
                    .values
                    .entry(value_key)
                    .or_default()
                    .push(c);
                true
            }
            KeyCode::Backspace => {
                self.computations.values.entry(value_key).or_default().pop();
                true
            }
            KeyCode::Up => {
                self.computations.field = field.saturating_sub(1);
                true
            }
            KeyCode::Down => {
                self.computations.field = (field + 1).min(params.len() - 1);
                true
            }
            _ => false,
        }
    }

    // -- Navigation -------------------------------------------------------

    #[allow(clippy::too_many_lines)]
    fn navigate(&mut self, action: Action) {
        match self.tab {
            Tab::Explorer => match self.explorer.pane {
                ExplorerPane::Tree => self.tree_navigate(action),
                ExplorerPane::Viewer => self.viewer_navigate(action),
            },
            Tab::Graph => self.graph_navigate(action),
            Tab::Trust => self.trust_navigate(action),
            Tab::Computations => self.computations_navigate(action),
        }
    }

    fn tree_navigate(&mut self, action: Action) {
        let rows = self.tree_rows();
        if rows.is_empty() {
            return;
        }
        let current = self
            .explorer
            .selected
            .as_ref()
            .and_then(|sel| rows.iter().position(|row| &row.sel == sel))
            .unwrap_or(0);
        let mut next = current;
        match action {
            Action::Up => next = current.saturating_sub(1),
            Action::Down => next = (current + 1).min(rows.len() - 1),
            Action::PageUp => next = current.saturating_sub(10),
            Action::PageDown => next = (current + 10).min(rows.len() - 1),
            Action::Home => next = 0,
            Action::End => next = rows.len() - 1,
            Action::Left => {
                match &rows[current].sel {
                    TreeSel::Dir(path) if !self.explorer.collapsed.contains(path) => {
                        self.explorer.collapsed.insert(path.clone());
                    }
                    sel => {
                        // Jump to the parent directory row.
                        let parent = match sel {
                            TreeSel::Concept(id) => id.parent().map(|p| p.to_string()),
                            TreeSel::Dir(path) => {
                                path.rsplit_once('/').map(|(parent, _)| parent.to_string())
                            }
                            TreeSel::File(_) => None,
                        };
                        if let Some(parent) = parent
                            && let Some(ix) = rows
                                .iter()
                                .position(|row| row.sel == TreeSel::Dir(parent.clone()))
                        {
                            next = ix;
                        }
                    }
                }
            }
            Action::Right => match &rows[current].sel {
                TreeSel::Dir(path) => {
                    self.explorer.collapsed.remove(path);
                }
                _ => self.explorer.pane = ExplorerPane::Viewer,
            },
            Action::Activate => match &rows[current].sel {
                TreeSel::Dir(path) => {
                    if self.explorer.collapsed.contains(path) {
                        self.explorer.collapsed.remove(path);
                    } else {
                        self.explorer.collapsed.insert(path.clone());
                    }
                }
                _ => self.explorer.pane = ExplorerPane::Viewer,
            },
            _ => {}
        }
        if next != current {
            self.select_row(&rows[next].sel);
        }
    }

    fn select_row(&mut self, sel: &TreeSel) {
        self.explorer.selected = Some(sel.clone());
        self.explorer.scroll = 0;
        self.explorer.focused_link = None;
    }

    fn viewer_navigate(&mut self, action: Action) {
        let max = self.explorer.max_scroll.get();
        let scroll = &mut self.explorer.scroll;
        match action {
            Action::Up => *scroll = scroll.saturating_sub(1),
            Action::Down => *scroll = (*scroll + 1).min(max),
            Action::PageUp => *scroll = scroll.saturating_sub(20),
            Action::PageDown => *scroll = (*scroll + 20).min(max),
            Action::HalfUp => *scroll = scroll.saturating_sub(10),
            Action::HalfDown => *scroll = (*scroll + 10).min(max),
            Action::Home => *scroll = 0,
            Action::End => *scroll = max,
            Action::Left => self.explorer.pane = ExplorerPane::Tree,
            Action::Activate => self.follow_focused_link(),
            _ => {}
        }
    }

    fn cycle_link(&mut self, forward: bool) {
        if self.tab != Tab::Explorer {
            return;
        }
        self.explorer.pane = ExplorerPane::Viewer;
        // The number of links is only known to the renderer; the view stores
        // it alongside max_scroll. We advance optimistically and let the view
        // clamp via the cache key; link count lives in the render cache.
        let count = self.explorer_link_count();
        if count == 0 {
            self.explorer.focused_link = None;
            return;
        }
        let next = match self.explorer.focused_link {
            None if forward => 0,
            None => count - 1,
            Some(i) if forward => (i + 1) % count,
            Some(i) => (i + count - 1) % count,
        };
        self.explorer.focused_link = Some(next);
    }

    /// Renders the selected concept once (uncached) to count its links.
    fn explorer_link_count(&self) -> usize {
        let Some(snapshot) = &self.snapshot else {
            return 0;
        };
        let Some(TreeSel::Concept(id)) = &self.explorer.selected else {
            return 0;
        };
        let Some(concept) = snapshot.bundle.get(id) else {
            return 0;
        };
        crate::markdown::render_document(&concept.document.body, 80, &self.theme, None)
            .links
            .len()
    }

    fn follow_focused_link(&mut self) {
        let Some(snapshot) = self.snapshot.clone() else {
            return;
        };
        let Some(TreeSel::Concept(id)) = self.explorer.selected.clone() else {
            return;
        };
        let Some(concept) = snapshot.bundle.get(&id) else {
            return;
        };
        let Some(focus) = self.explorer.focused_link else {
            return;
        };
        let rendered =
            crate::markdown::render_document(&concept.document.body, 80, &self.theme, None);
        let Some(target) = rendered.links.get(focus) else {
            return;
        };
        match &target.kind {
            crate::markdown::FocusKind::Footnote(label) => {
                if let Some(&line) = rendered.footnote_defs.get(label) {
                    self.explorer.scroll = line;
                }
            }
            crate::markdown::FocusKind::Link { target, kind, .. } => {
                let link = okf_core::Link {
                    text: String::new(),
                    target: target.clone(),
                    kind: *kind,
                };
                if let Some(resolved) = link
                    .resolve_all(&id)
                    .into_iter()
                    .find(|t| snapshot.bundle.contains(t))
                {
                    self.open_concept(&resolved);
                } else if *kind == okf_core::LinkKind::External {
                    self.toast(format!("external: {target}"), false);
                } else {
                    self.toast(format!("✗ broken link: {target}"), true);
                }
            }
        }
    }

    /// Jumps the explorer to a concept, recording history.
    pub fn open_concept(&mut self, id: &ConceptId) {
        if let Some(TreeSel::Concept(current)) = &self.explorer.selected
            && current != id
        {
            self.explorer.history.push(current.clone());
            self.explorer.future.clear();
        }
        self.tab = Tab::Explorer;
        self.explorer.pane = ExplorerPane::Viewer;
        self.select_row(&TreeSel::Concept(id.clone()));
        self.reveal_in_tree(id);
    }

    /// Expands ancestors so the selection is visible in the tree.
    fn reveal_in_tree(&mut self, id: &ConceptId) {
        let mut prefix = String::new();
        let segments = id.segments();
        for segment in &segments[..segments.len().saturating_sub(1)] {
            if !prefix.is_empty() {
                prefix.push('/');
            }
            prefix.push_str(segment);
            self.explorer.collapsed.remove(&prefix);
        }
    }

    fn nav_back(&mut self) {
        if let Some(previous) = self.explorer.history.pop() {
            if let Some(TreeSel::Concept(current)) = &self.explorer.selected {
                self.explorer.future.push(current.clone());
            }
            self.select_row(&TreeSel::Concept(previous.clone()));
            self.reveal_in_tree(&previous);
        }
    }

    fn nav_forward(&mut self) {
        if let Some(next) = self.explorer.future.pop() {
            if let Some(TreeSel::Concept(current)) = &self.explorer.selected {
                self.explorer.history.push(current.clone());
            }
            self.select_row(&TreeSel::Concept(next.clone()));
            self.reveal_in_tree(&next);
        }
    }

    fn graph_navigate(&mut self, action: Action) {
        let step = 0.15 / self.graph.zoom;
        match action {
            Action::Up => self.graph.pan.1 += step,
            Action::Down => self.graph.pan.1 -= step,
            Action::Left => self.graph.pan.0 -= step,
            Action::Right => self.graph.pan.0 += step,
            Action::Activate => {
                if let Some(id) = self.selected_concept() {
                    self.open_concept(&id);
                }
            }
            _ => {}
        }
    }

    fn cycle_node(&mut self, forward: bool) {
        let Some(snapshot) = self.snapshot.clone() else {
            return;
        };
        let included = self.graph_included(&snapshot);
        // Reading order over layout positions gives a stable, spatial cycle.
        let mut visible: Vec<(usize, (f64, f64))> = snapshot
            .graph
            .nodes
            .iter()
            .enumerate()
            .filter(|(i, _)| included[*i])
            .map(|(i, node)| {
                (
                    i,
                    self.graph
                        .layout
                        .positions
                        .get(&node.key)
                        .copied()
                        .unwrap_or((0.0, 0.0)),
                )
            })
            .collect();
        if visible.is_empty() {
            return;
        }
        visible.sort_by(|a, b| {
            b.1.1
                .partial_cmp(&a.1.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(
                    a.1.0
                        .partial_cmp(&b.1.0)
                        .unwrap_or(std::cmp::Ordering::Equal),
                )
        });
        let current = self.graph.selected.as_ref().and_then(|key| {
            visible
                .iter()
                .position(|(i, _)| &snapshot.graph.nodes[*i].key == key)
        });
        let next_pos = match current {
            None => 0,
            Some(p) if forward => (p + 1) % visible.len(),
            Some(p) => (p + visible.len() - 1) % visible.len(),
        };
        let node = &snapshot.graph.nodes[visible[next_pos].0];
        self.graph.selected = Some(node.key.clone());
        // Center the selection.
        if let Some(&(x, y)) = self.graph.layout.positions.get(&node.key) {
            self.graph.pan = (x, y);
        }
    }

    fn cycle_focus_mode(&mut self) {
        let center = self
            .graph
            .selected
            .clone()
            .or_else(|| self.graph.focus.as_ref().map(|(key, _)| key.clone()));
        let Some(center) = center else {
            self.toast("select a node first (Tab)", false);
            return;
        };
        self.graph.focus = match &self.graph.focus {
            None => Some((center, 1)),
            Some((_, k)) if *k < 3 => Some((center, k + 1)),
            Some(_) => None,
        };
    }

    fn trust_navigate(&mut self, action: Action) {
        let queue_len = self.filtered_queue().len();
        match self.trust.panel {
            TrustPanel::Queue => match action {
                Action::Up => self.trust.queue_sel = self.trust.queue_sel.saturating_sub(1),
                Action::Down => {
                    self.trust.queue_sel =
                        (self.trust.queue_sel + 1).min(queue_len.saturating_sub(1));
                }
                Action::Home => self.trust.queue_sel = 0,
                Action::End => self.trust.queue_sel = queue_len.saturating_sub(1),
                Action::Activate => {
                    if let Some(item) = self.filtered_queue().get(self.trust.queue_sel) {
                        let id = item.id.clone();
                        self.open_concept(&id);
                    }
                }
                _ => {}
            },
            TrustPanel::Activity => {
                if action == Action::Activate {
                    self.overlays.push(Overlay::LogView(0));
                }
            }
            panel => {
                let rows = match panel {
                    TrustPanel::TrustBars | TrustPanel::FreshnessBars => 3,
                    _ => 4,
                };
                match action {
                    Action::Up => self.trust.bar_sel = self.trust.bar_sel.saturating_sub(1),
                    Action::Down => self.trust.bar_sel = (self.trust.bar_sel + 1).min(rows - 1),
                    Action::Activate => {
                        let cohort = match panel {
                            TrustPanel::TrustBars => Cohort::Tier(match self.trust.bar_sel {
                                2 => TrustTier::Unverified,
                                1 => TrustTier::MachineConfirmed,
                                _ => TrustTier::HumanReviewed,
                            }),
                            TrustPanel::LifecycleBars => Cohort::Status(match self.trust.bar_sel {
                                0 => 1, // stable listed first
                                1 => 0,
                                other => other,
                            }),
                            _ => Cohort::Fresh(self.trust.bar_sel),
                        };
                        // Selecting the active cohort again clears it.
                        self.trust.cohort = if self.trust.cohort == Some(cohort) {
                            None
                        } else {
                            Some(cohort)
                        };
                        self.trust.queue_sel = 0;
                    }
                    _ => {}
                }
            }
        }
    }

    fn computations_navigate(&mut self, action: Action) {
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        let count = snapshot.contracts.len();
        match self.computations.pane {
            CompPane::List => match action {
                Action::Up => {
                    self.computations.selected = self.computations.selected.saturating_sub(1);
                }
                Action::Down => {
                    self.computations.selected =
                        (self.computations.selected + 1).min(count.saturating_sub(1));
                }
                Action::Activate => {
                    if let Some(id) = self.selected_concept() {
                        self.open_concept(&id);
                    }
                }
                _ => {}
            },
            CompPane::Form => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Overlays.
// ---------------------------------------------------------------------------

/// One row of the merged diagnostics overlay.
#[derive(Clone, Debug)]
pub struct DiagRow {
    /// `true` when the row comes from lint rather than validation.
    pub from_lint: bool,
    /// Severity.
    pub severity: okf_validator::Severity,
    /// The concept, when attributable.
    pub concept: Option<ConceptId>,
    /// The file, when attributable.
    pub path: Option<PathBuf>,
    /// The message.
    pub message: String,
    /// Whether `okf fix` can remediate it.
    pub fixable: bool,
}

/// The palette's computed results.
#[derive(Debug)]
pub enum PaletteResults {
    /// Omnisearch hits.
    Search(Vec<crate::search::SearchHit>),
    /// Command rows: `(label, key hint, action)`.
    Commands(Vec<(String, String, Action)>),
}

/// The curated command-mode entries: every studio action as a fuzzy-searchable
/// verb, each showing its direct keybinding as passive training.
fn command_entries() -> Vec<(&'static str, Action)> {
    vec![
        ("stamp verification", Action::Verify),
        ("extend stale_after…", Action::ExtendStale),
        ("move concept…", Action::MoveOrRename),
        ("remove concept…", Action::Remove),
        ("merge concept into…", Action::Merge),
        ("split section…", Action::SplitSection),
        ("new concept…", Action::NewConcept),
        ("fix all safe issues", Action::FixAll),
        ("diagnostics", Action::OpenDiagnostics),
        ("pin evaluation date (--today)…", Action::PinToday),
        ("open activity log", Action::OpenLog),
        ("open in $EDITOR", Action::OpenEditor),
        ("reload bundle", Action::Reload),
        ("toggle: color graph by next dimension", Action::CycleColor),
        ("toggle: graph source nodes", Action::ToggleSources),
        ("toggle: graph broken targets", Action::ToggleBroken),
        ("toggle: graph derivation edges", Action::ToggleDerivations),
        ("cycle graph layout", Action::CycleLayout),
        ("toggle raw YAML frontmatter", Action::ToggleRawYaml),
        ("help / cheatsheet", Action::OpenHelp),
        ("quit", Action::Quit),
    ]
}

impl App {
    /// Computes the palette's result rows for its current input.
    #[must_use]
    pub fn palette_results(&self, state: &PaletteState) -> PaletteResults {
        if state.mode() == PaletteMode::Command {
            let query = state.input.trim_start().trim_start_matches('>').trim();
            let mut rows: Vec<(i32, (String, String, Action))> = Vec::new();
            for (label, action) in command_entries() {
                let score = if query.is_empty() {
                    Some((0, Vec::new()))
                } else {
                    crate::search::fuzzy_match(query, label)
                };
                if let Some((score, _)) = score {
                    let key = crate::keymap::bindings()
                        .iter()
                        .find(|b| b.action == action && b.primary)
                        .map_or(String::new(), |b| b.label.to_string());
                    rows.push((score, (label.to_string(), key, action)));
                }
            }
            rows.sort_by_key(|a| std::cmp::Reverse(a.0));
            PaletteResults::Commands(rows.into_iter().map(|(_, row)| row).collect())
        } else {
            let hits = self.snapshot.as_ref().map_or_else(Vec::new, |snapshot| {
                snapshot.search.search(&state.input, 50)
            });
            PaletteResults::Search(hits)
        }
    }

    /// The merged validation + lint rows for a diagnostics filter.
    #[must_use]
    pub fn diag_rows(&self, filter: DiagFilter) -> Vec<DiagRow> {
        let Some(snapshot) = &self.snapshot else {
            return Vec::new();
        };
        let mut rows = Vec::new();
        if filter != DiagFilter::Lint {
            for d in &snapshot.validation.diagnostics {
                let include = match filter {
                    DiagFilter::Errors => d.severity == okf_validator::Severity::Error,
                    DiagFilter::Warnings => d.severity == okf_validator::Severity::Warning,
                    _ => true,
                };
                if include {
                    rows.push(DiagRow {
                        from_lint: false,
                        severity: d.severity,
                        concept: d.concept.clone(),
                        path: d.path.clone(),
                        message: d.message.clone(),
                        fixable: d.fixable,
                    });
                }
            }
        }
        if matches!(filter, DiagFilter::All | DiagFilter::Lint) {
            for d in &snapshot.lint.diagnostics {
                rows.push(DiagRow {
                    from_lint: true,
                    severity: d.severity,
                    concept: d.concept.clone(),
                    path: d.path.clone(),
                    message: d.message.clone(),
                    fixable: d.fixable,
                });
            }
        }
        // Errors first, then warnings, then the rest.
        rows.sort_by_key(|a| std::cmp::Reverse(a.severity));
        rows
    }

    #[allow(clippy::too_many_lines)]
    fn overlay_key(&mut self, key: KeyEvent) {
        if key.code == KeyCode::Esc {
            self.overlays.pop();
            return;
        }
        let Some(overlay) = self.overlays.pop() else {
            return;
        };
        match overlay {
            Overlay::Palette(state) => self.palette_key(state, key),
            Overlay::Diagnostics(state) => self.diagnostics_key(state, key),
            Overlay::Help(scroll) => {
                let next = match key.code {
                    KeyCode::Up | KeyCode::Char('k') => scroll.saturating_sub(1),
                    KeyCode::Down | KeyCode::Char('j') => scroll + 1,
                    KeyCode::PageUp => scroll.saturating_sub(20),
                    KeyCode::PageDown => scroll + 20,
                    KeyCode::Char('q' | '?') | KeyCode::Enter => {
                        return;
                    }
                    _ => scroll,
                };
                self.overlays.push(Overlay::Help(next));
            }
            Overlay::Confirm(state) => {
                if key.code == KeyCode::Enter {
                    match &state.action {
                        ConfirmAction::Verify(id) => {
                            self.loading = LoadPhase::Applying;
                            self.send(Command::StampVerification(id.clone()));
                        }
                    }
                } else {
                    self.overlays.push(Overlay::Confirm(state));
                }
            }
            Overlay::DatePicker(state) => self.date_picker_key(state, key),
            Overlay::Refactor(state) => self.refactor_key(*state, key),
            Overlay::NewConcept(state) => self.new_concept_key(state, key),
            Overlay::FixPreview(state) => self.fix_preview_key(state, key),
            Overlay::LogView(scroll) => {
                let next = match key.code {
                    KeyCode::Up | KeyCode::Char('k') => scroll.saturating_sub(1),
                    KeyCode::Down | KeyCode::Char('j') => scroll + 1,
                    KeyCode::PageUp => scroll.saturating_sub(20),
                    KeyCode::PageDown => scroll + 20,
                    KeyCode::Char('q') => return,
                    _ => scroll,
                };
                self.overlays.push(Overlay::LogView(next));
            }
            Overlay::Outline(state) => self.outline_key(state, key),
        }
    }

    fn palette_key(&mut self, mut state: PaletteState, key: KeyEvent) {
        match key.code {
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                state.input.push(c);
                state.sel = 0;
                self.overlays.push(Overlay::Palette(state));
            }
            KeyCode::Backspace => {
                state.input.pop();
                state.sel = 0;
                self.overlays.push(Overlay::Palette(state));
            }
            KeyCode::Up => {
                state.sel = state.sel.saturating_sub(1);
                self.overlays.push(Overlay::Palette(state));
            }
            KeyCode::Down => {
                state.sel += 1;
                self.overlays.push(Overlay::Palette(state));
            }
            KeyCode::Enter => match self.palette_results(&state) {
                PaletteResults::Search(hits) => {
                    if let Some(hit) = hits.get(state.sel.min(hits.len().saturating_sub(1))) {
                        let id = hit.id.clone();
                        if self.tab == Tab::Graph {
                            let key = id.to_string();
                            if let Some(&(x, y)) = self.graph.layout.positions.get(&key) {
                                self.graph.pan = (x, y);
                            }
                            self.graph.selected = Some(key);
                        } else {
                            self.open_concept(&id);
                        }
                    }
                }
                PaletteResults::Commands(commands) => {
                    if let Some((_, _, action)) =
                        commands.get(state.sel.min(commands.len().saturating_sub(1)))
                    {
                        self.run_action(*action);
                    }
                }
            },
            _ => self.overlays.push(Overlay::Palette(state)),
        }
    }

    fn diagnostics_key(&mut self, mut state: DiagnosticsState, key: KeyEvent) {
        let rows = self.diag_rows(state.filter);
        match key.code {
            KeyCode::Char('a') => state.filter = DiagFilter::All,
            KeyCode::Char('e') => state.filter = DiagFilter::Errors,
            KeyCode::Char('w') => state.filter = DiagFilter::Warnings,
            KeyCode::Char('l') => state.filter = DiagFilter::Lint,
            KeyCode::Up | KeyCode::Char('k') => state.sel = state.sel.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => {
                state.sel = (state.sel + 1).min(rows.len().saturating_sub(1));
            }
            KeyCode::Enter => {
                if let Some(row) = rows.get(state.sel)
                    && let Some(id) = row.concept.clone()
                {
                    self.open_concept(&id);
                    return;
                }
            }
            KeyCode::Char('f') => {
                if let Some(row) = rows.get(state.sel) {
                    let path = row.path.clone().or_else(|| {
                        self.snapshot.as_ref().and_then(|s| {
                            row.concept.as_ref().map(|id| id.to_path(s.bundle.root()))
                        })
                    });
                    if let Some(path) = path {
                        self.loading = LoadPhase::Applying;
                        self.send(Command::ApplyFixFile(path));
                    } else {
                        self.toast("no file to fix", true);
                    }
                }
            }
            KeyCode::Char('F') => self.send(Command::PreviewFix),
            KeyCode::Char('t') => {
                let today = self
                    .snapshot
                    .as_ref()
                    .map_or_else(String::new, |s| s.today.to_string());
                self.overlays.push(Overlay::Diagnostics(state));
                self.overlays.push(Overlay::DatePicker(DatePickerState {
                    id: None,
                    input: today,
                }));
                return;
            }
            _ => {}
        }
        state.sel = state
            .sel
            .min(self.diag_rows(state.filter).len().saturating_sub(1));
        self.overlays.push(Overlay::Diagnostics(state));
    }

    fn date_picker_key(&mut self, mut state: DatePickerState, key: KeyEvent) {
        let shift = |input: &str, days: i64| -> Option<String> {
            Date::parse(input.trim())
                .map(|d| Date::from_days_since_epoch(d.days_since_epoch() + days).to_string())
        };
        match key.code {
            KeyCode::Char(c) if c.is_ascii_digit() || c == '-' => state.input.push(c),
            KeyCode::Backspace => {
                state.input.pop();
            }
            KeyCode::Up => {
                if let Some(next) = shift(&state.input, 1) {
                    state.input = next;
                }
            }
            KeyCode::Down => {
                if let Some(next) = shift(&state.input, -1) {
                    state.input = next;
                }
            }
            KeyCode::PageUp => {
                if let Some(next) = shift(&state.input, 30) {
                    state.input = next;
                }
            }
            KeyCode::PageDown => {
                if let Some(next) = shift(&state.input, -30) {
                    state.input = next;
                }
            }
            KeyCode::Enter => {
                match (&state.id, Date::parse(state.input.trim())) {
                    (Some(id), Some(date)) => {
                        self.loading = LoadPhase::Applying;
                        self.send(Command::SetStaleAfter(id.clone(), date));
                    }
                    (None, Some(date)) => {
                        self.loading = LoadPhase::Reloading;
                        self.send(Command::SetToday(Some(date)));
                        self.toast(format!("today pinned to {date}"), false);
                    }
                    (None, None) if state.input.trim().is_empty() => {
                        self.loading = LoadPhase::Reloading;
                        self.send(Command::SetToday(None));
                        self.toast("today unpinned (wall clock)", false);
                    }
                    _ => {
                        self.toast("not a YYYY-MM-DD date", true);
                        self.overlays.push(Overlay::DatePicker(state));
                    }
                }
                return;
            }
            _ => {}
        }
        self.overlays.push(Overlay::DatePicker(state));
    }

    #[allow(clippy::too_many_lines)]
    fn refactor_key(&mut self, mut state: RefactorState, key: KeyEvent) {
        let awaiting_decision = matches!(
            state.preview,
            Some(Err(RefactorError::HasInboundLinks { .. }))
        ) || state.choice != RemoveChoice::Plain;
        let target_exists = matches!(
            state.preview,
            Some(Err(RefactorError::ConceptAlreadyExists(_)))
        );
        match key.code {
            KeyCode::Enter => {
                if matches!(state.preview, Some(Ok(_)))
                    && let Some(op) = build_op(&state)
                {
                    self.loading = LoadPhase::Applying;
                    self.send(Command::Apply(op));
                    return;
                }
                state.needs_preview = true;
            }
            KeyCode::Tab | KeyCode::BackTab => {
                let fields = match state.verb {
                    VerbKind::Split => 2,
                    VerbKind::Remove if state.choice == RemoveChoice::Redirect => 2,
                    _ => 1,
                };
                state.field = (state.field + 1) % fields;
            }
            KeyCode::Char('r') if state.verb == VerbKind::Remove && awaiting_decision => {
                state.choice = RemoveChoice::Redirect;
                state.field = 1;
                state.needs_preview = true;
            }
            KeyCode::Char('u') if state.verb == VerbKind::Remove && awaiting_decision => {
                state.choice = RemoveChoice::Unlink;
                state.needs_preview = true;
            }
            KeyCode::Char('f') if state.verb == VerbKind::Remove && awaiting_decision => {
                state.choice = RemoveChoice::Force;
                state.needs_preview = true;
            }
            KeyCode::Char('o')
                if matches!(state.verb, VerbKind::Move | VerbKind::Split) && target_exists =>
            {
                state.force = true;
                state.needs_preview = true;
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                let field = if state.field == 0 {
                    &mut state.input
                } else {
                    &mut state.extra
                };
                field.push(c);
                state.force = false;
                state.needs_preview = true;
            }
            KeyCode::Backspace => {
                let field = if state.field == 0 {
                    &mut state.input
                } else {
                    &mut state.extra
                };
                field.pop();
                state.force = false;
                state.needs_preview = true;
            }
            _ => {}
        }
        self.overlays.push(Overlay::Refactor(Box::new(state)));
    }

    fn new_concept_key(&mut self, mut state: NewConceptState, key: KeyEvent) {
        match key.code {
            KeyCode::Tab | KeyCode::Down => state.field = (state.field + 1) % 3,
            KeyCode::BackTab | KeyCode::Up => state.field = (state.field + 2) % 3,
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                state.fields[state.field].push(c);
            }
            KeyCode::Backspace => {
                state.fields[state.field].pop();
            }
            KeyCode::Enter => {
                let path = state.fields[0].trim().to_string();
                if path.is_empty() || path.ends_with('/') {
                    self.toast("enter a concept path", true);
                    self.overlays.push(Overlay::NewConcept(state));
                    return;
                }
                let type_ = if state.fields[1].trim().is_empty() {
                    "Concept".to_string()
                } else {
                    state.fields[1].trim().to_string()
                };
                let title = if state.fields[2].trim().is_empty() {
                    None
                } else {
                    Some(state.fields[2].trim().to_string())
                };
                self.loading = LoadPhase::Applying;
                self.send(Command::CreateConcept {
                    rel_path: path,
                    type_,
                    title,
                });
                return;
            }
            _ => {}
        }
        self.overlays.push(Overlay::NewConcept(state));
    }

    fn fix_preview_key(&mut self, mut state: FixPreviewState, key: KeyEvent) {
        let changed_count = state.report.files.iter().filter(|f| f.changed).count();
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => state.scroll = state.scroll.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => state.scroll += 1,
            KeyCode::PageUp => state.scroll = state.scroll.saturating_sub(20),
            KeyCode::PageDown => state.scroll += 20,
            KeyCode::Left | KeyCode::BackTab => {
                state.file_sel = state.file_sel.saturating_sub(1);
                state.scroll = 0;
            }
            KeyCode::Right | KeyCode::Tab => {
                state.file_sel = (state.file_sel + 1).min(changed_count.saturating_sub(1));
                state.scroll = 0;
            }
            KeyCode::Enter => {
                self.loading = LoadPhase::Applying;
                self.send(Command::ApplyFix);
                return;
            }
            _ => {}
        }
        self.overlays.push(Overlay::FixPreview(state));
    }

    fn outline_key(&mut self, mut state: OutlineState, key: KeyEvent) {
        let headings: Vec<(usize, String)> = self
            .snapshot
            .as_ref()
            .and_then(|s| s.meta(&state.id))
            .map(|m| m.headings.clone())
            .unwrap_or_default();
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => state.sel = state.sel.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => {
                state.sel = (state.sel + 1).min(headings.len().saturating_sub(1));
            }
            KeyCode::Enter => {
                if let Some((_, text)) = headings.get(state.sel) {
                    self.scroll_to_heading(&state.id, text);
                }
                return;
            }
            KeyCode::Char('s') => {
                if let Some((_, text)) = headings.get(state.sel) {
                    let id = state.id.clone();
                    self.open_refactor(VerbKind::Split, id, Some(text.clone()));
                }
                return;
            }
            KeyCode::Char('r') | KeyCode::F(2) => {
                if let Some((_, text)) = headings.get(state.sel) {
                    let id = state.id.clone();
                    self.open_refactor(VerbKind::RenameSection, id, Some(text.clone()));
                }
                return;
            }
            _ => {}
        }
        self.overlays.push(Overlay::Outline(state));
    }

    /// Scrolls the viewer to a heading (rendered-line index approximated at a
    /// standard width; the view clamps).
    fn scroll_to_heading(&mut self, id: &ConceptId, heading: &str) {
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        let Some(concept) = snapshot.bundle.get(id) else {
            return;
        };
        let rendered =
            crate::markdown::render_document(&concept.document.body, 80, &self.theme, None);
        if let Some(pos) = rendered.headings.iter().find(|h| h.text == heading) {
            self.explorer.pane = ExplorerPane::Viewer;
            self.explorer.scroll = pos.line;
        }
    }
}
