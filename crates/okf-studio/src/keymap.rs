//! The keybinding table: context → key → action, with a primary/alias flag.
//!
//! Dispatch, the hint bar, and the help overlay are all generated from this
//! one table, so they can never drift. Standard keys (arrows, `F2`, `Del`,
//! `Enter`) are the primary bindings the UI advertises; the vim-style letters
//! are complementary aliases bound on top, never instead.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Where a binding applies. Contexts are consulted from most to least
/// specific: the focused pane's context, then [`Context::Concept`] when a
/// concept is selected, then [`Context::Global`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Context {
    /// Always active.
    Global,
    /// Explorer with the tree pane focused.
    Tree,
    /// Explorer with the document pane focused.
    Viewer,
    /// The graph workspace.
    Graph,
    /// The mission-control workspace.
    Trust,
    /// The computations workspace.
    Computations,
    /// Verbs on the currently selected concept, any workspace.
    Concept,
}

/// Every dispatchable studio action.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// Switch to workspace `0..=3`.
    SwitchTab(usize),
    /// Open the omnisearch / command palette.
    OpenPalette,
    /// Open the diagnostics overlay.
    OpenDiagnostics,
    /// Open the help cheatsheet.
    OpenHelp,
    /// Force a bundle reload.
    Reload,
    /// Open the selected file in `$EDITOR`.
    OpenEditor,
    /// Quit studio.
    Quit,
    /// Move selection / scroll up.
    Up,
    /// Move selection / scroll down.
    Down,
    /// Collapse / focus left.
    Left,
    /// Expand / focus right.
    Right,
    /// Page up.
    PageUp,
    /// Page down.
    PageDown,
    /// Half page down (vim `Ctrl+D`).
    HalfDown,
    /// Half page up (vim `Ctrl+U`).
    HalfUp,
    /// Jump to start.
    Home,
    /// Jump to end.
    End,
    /// Activate the selection (open / follow / filter).
    Activate,
    /// Navigate back in history.
    Back,
    /// Navigate forward in history.
    Forward,
    /// Focus the next link in the document.
    NextLink,
    /// Focus the previous link in the document.
    PrevLink,
    /// Toggle raw-YAML frontmatter view.
    ToggleRawYaml,
    /// Open the outline jump list.
    Outline,
    /// Cycle the inspector tab.
    CycleInspector,
    /// Stamp a verification on the selected concept.
    Verify,
    /// Extend the selected concept's `stale_after`.
    ExtendStale,
    /// Move / rename the selection (rename section on a heading).
    MoveOrRename,
    /// Remove the selected concept.
    Remove,
    /// Merge the selected concept into another.
    Merge,
    /// Split the focused section into a new concept.
    SplitSection,
    /// Create a new concept here.
    NewConcept,
    /// Zoom the graph in.
    ZoomIn,
    /// Zoom the graph out.
    ZoomOut,
    /// Select the next graph node.
    NextNode,
    /// Select the previous graph node.
    PrevNode,
    /// Toggle / widen egocentric focus mode.
    FocusMode,
    /// Cycle the node coloring dimension.
    CycleColor,
    /// Toggle external source nodes.
    ToggleSources,
    /// Toggle broken-target nodes.
    ToggleBroken,
    /// Toggle derivation edges.
    ToggleDerivations,
    /// Pause / resume the layout simulation.
    PauseLayout,
    /// Filter graph nodes by fuzzy query.
    GraphFilter,
    /// Cycle the layout algorithm.
    CycleLayout,
    /// Cycle panel focus within the workspace.
    CyclePanel,
    /// Cycle the attention-queue sort.
    CycleSort,
    /// Copy the invocation sketch (OSC 52).
    CopySketch,
    /// Preview and apply all safe fixes (palette-only).
    FixAll,
    /// Pin the evaluation date (palette-only).
    PinToday,
    /// Open the full activity log (palette-only).
    OpenLog,
}

/// One row of the keybinding table.
pub struct Binding {
    /// The context the binding applies in.
    pub context: Context,
    /// The key code.
    pub code: KeyCode,
    /// Required modifiers (`SHIFT` is ignored for `Char` codes).
    pub mods: KeyModifiers,
    /// The action dispatched.
    pub action: Action,
    /// Whether this is the advertised binding (vs. a vim-style alias).
    pub primary: bool,
    /// Display label for the hint bar and help overlay.
    pub label: &'static str,
    /// One-line description.
    pub help: &'static str,
}

const fn b(
    context: Context,
    code: KeyCode,
    mods: KeyModifiers,
    action: Action,
    primary: bool,
    label: &'static str,
    help: &'static str,
) -> Binding {
    Binding {
        context,
        code,
        mods,
        action,
        primary,
        label,
        help,
    }
}

const NONE: KeyModifiers = KeyModifiers::NONE;
const CTRL: KeyModifiers = KeyModifiers::CONTROL;

/// The full binding table, in help-overlay order.
#[must_use]
pub const fn bindings() -> &'static [Binding] {
    BINDINGS
}

#[allow(clippy::too_many_lines)]
const BINDINGS: &[Binding] = {
    use Action as A;
    use Context as C;
    use KeyCode as K;
    &[
        // Global.
        b(
            C::Global,
            K::Char('1'),
            NONE,
            A::SwitchTab(0),
            true,
            "1",
            "Explorer workspace",
        ),
        b(
            C::Global,
            K::Char('2'),
            NONE,
            A::SwitchTab(1),
            true,
            "2",
            "Graph workspace",
        ),
        b(
            C::Global,
            K::Char('3'),
            NONE,
            A::SwitchTab(2),
            true,
            "3",
            "Trust workspace",
        ),
        b(
            C::Global,
            K::Char('4'),
            NONE,
            A::SwitchTab(3),
            true,
            "4",
            "Computations workspace",
        ),
        b(
            C::Global,
            K::Char('/'),
            NONE,
            A::OpenPalette,
            true,
            "/",
            "Omnisearch (> for commands)",
        ),
        b(
            C::Global,
            K::Char('p'),
            CTRL,
            A::OpenPalette,
            false,
            "Ctrl+P",
            "Command palette",
        ),
        b(
            C::Global,
            K::Char('!'),
            NONE,
            A::OpenDiagnostics,
            true,
            "!",
            "Diagnostics & fixes",
        ),
        b(
            C::Global,
            K::Char('?'),
            NONE,
            A::OpenHelp,
            true,
            "?",
            "Help / cheatsheet",
        ),
        b(
            C::Global,
            K::Char('R'),
            NONE,
            A::Reload,
            true,
            "R",
            "Reload bundle",
        ),
        b(
            C::Global,
            K::Char('e'),
            NONE,
            A::OpenEditor,
            true,
            "e",
            "Open in $EDITOR",
        ),
        b(C::Global, K::Char('q'), NONE, A::Quit, true, "q", "Quit"),
        // Concept verbs.
        b(
            C::Concept,
            K::Char('v'),
            NONE,
            A::Verify,
            true,
            "v",
            "Stamp verification",
        ),
        b(
            C::Concept,
            K::Char('x'),
            NONE,
            A::ExtendStale,
            true,
            "x",
            "Extend stale_after",
        ),
        b(
            C::Concept,
            K::F(2),
            NONE,
            A::MoveOrRename,
            true,
            "F2",
            "Move / rename",
        ),
        b(
            C::Concept,
            K::Char('m'),
            NONE,
            A::MoveOrRename,
            false,
            "m",
            "Move / rename",
        ),
        b(
            C::Concept,
            K::Delete,
            NONE,
            A::Remove,
            true,
            "Del",
            "Remove concept",
        ),
        b(
            C::Concept,
            K::Char('M'),
            NONE,
            A::Merge,
            true,
            "M",
            "Merge into…",
        ),
        b(
            C::Concept,
            K::Char('n'),
            NONE,
            A::NewConcept,
            true,
            "n",
            "New concept here",
        ),
        // Tree pane.
        b(C::Tree, K::Up, NONE, A::Up, true, "↑", "Previous row"),
        b(C::Tree, K::Down, NONE, A::Down, true, "↓", "Next row"),
        b(
            C::Tree,
            K::Char('k'),
            NONE,
            A::Up,
            false,
            "k",
            "Previous row",
        ),
        b(C::Tree, K::Char('j'), NONE, A::Down, false, "j", "Next row"),
        b(
            C::Tree,
            K::Left,
            NONE,
            A::Left,
            true,
            "←",
            "Collapse directory",
        ),
        b(
            C::Tree,
            K::Right,
            NONE,
            A::Right,
            true,
            "→",
            "Expand / focus document",
        ),
        b(
            C::Tree,
            K::Char('h'),
            NONE,
            A::Left,
            false,
            "h",
            "Collapse directory",
        ),
        b(
            C::Tree,
            K::Char('l'),
            NONE,
            A::Right,
            false,
            "l",
            "Expand / focus document",
        ),
        b(
            C::Tree,
            K::Enter,
            NONE,
            A::Activate,
            true,
            "⏎",
            "Open selection",
        ),
        b(C::Tree, K::Home, NONE, A::Home, true, "Home", "First row"),
        b(C::Tree, K::End, NONE, A::End, true, "End", "Last row"),
        b(C::Tree, K::PageUp, NONE, A::PageUp, true, "PgUp", "Page up"),
        b(
            C::Tree,
            K::PageDown,
            NONE,
            A::PageDown,
            true,
            "PgDn",
            "Page down",
        ),
        b(
            C::Tree,
            K::Char('d'),
            NONE,
            A::Remove,
            false,
            "d",
            "Remove concept",
        ),
        b(
            C::Tree,
            K::Char('i'),
            NONE,
            A::CycleInspector,
            true,
            "i",
            "Cycle inspector tab",
        ),
        // Document viewer.
        b(C::Viewer, K::Up, NONE, A::Up, true, "↑", "Scroll up"),
        b(C::Viewer, K::Down, NONE, A::Down, true, "↓", "Scroll down"),
        b(
            C::Viewer,
            K::Char('k'),
            NONE,
            A::Up,
            false,
            "k",
            "Scroll up",
        ),
        b(
            C::Viewer,
            K::Char('j'),
            NONE,
            A::Down,
            false,
            "j",
            "Scroll down",
        ),
        b(
            C::Viewer,
            K::PageUp,
            NONE,
            A::PageUp,
            true,
            "PgUp",
            "Page up",
        ),
        b(
            C::Viewer,
            K::PageDown,
            NONE,
            A::PageDown,
            true,
            "PgDn",
            "Page down",
        ),
        b(
            C::Viewer,
            K::Char('u'),
            CTRL,
            A::HalfUp,
            false,
            "Ctrl+U",
            "Half page up",
        ),
        b(
            C::Viewer,
            K::Char('d'),
            CTRL,
            A::HalfDown,
            false,
            "Ctrl+D",
            "Half page down",
        ),
        b(C::Viewer, K::Home, NONE, A::Home, true, "Home", "Top"),
        b(C::Viewer, K::End, NONE, A::End, true, "End", "Bottom"),
        b(C::Viewer, K::Char('g'), NONE, A::Home, false, "g", "Top"),
        b(C::Viewer, K::Char('G'), NONE, A::End, false, "G", "Bottom"),
        b(
            C::Viewer,
            K::Tab,
            NONE,
            A::NextLink,
            true,
            "Tab",
            "Focus next link",
        ),
        b(
            C::Viewer,
            K::BackTab,
            NONE,
            A::PrevLink,
            true,
            "S-Tab",
            "Focus previous link",
        ),
        b(
            C::Viewer,
            K::Enter,
            NONE,
            A::Activate,
            true,
            "⏎",
            "Follow focused link",
        ),
        b(C::Viewer, K::Backspace, NONE, A::Back, true, "⌫", "Back"),
        b(
            C::Viewer,
            K::Char('o'),
            CTRL,
            A::Back,
            false,
            "Ctrl+O",
            "Back",
        ),
        b(
            C::Viewer,
            K::Char('i'),
            CTRL,
            A::Forward,
            false,
            "Ctrl+I",
            "Forward",
        ),
        b(
            C::Viewer,
            K::Char('y'),
            NONE,
            A::ToggleRawYaml,
            true,
            "y",
            "Toggle raw YAML",
        ),
        b(
            C::Viewer,
            K::Char('o'),
            NONE,
            A::Outline,
            true,
            "o",
            "Outline jump list",
        ),
        b(
            C::Viewer,
            K::Char('i'),
            NONE,
            A::CycleInspector,
            true,
            "i",
            "Cycle inspector tab",
        ),
        b(C::Viewer, K::Left, NONE, A::Left, true, "←", "Focus tree"),
        b(
            C::Viewer,
            K::Char('s'),
            NONE,
            A::SplitSection,
            true,
            "s",
            "Split section out",
        ),
        // Graph.
        b(C::Graph, K::Up, NONE, A::Up, true, "↑", "Pan up"),
        b(C::Graph, K::Down, NONE, A::Down, true, "↓", "Pan down"),
        b(C::Graph, K::Left, NONE, A::Left, true, "←", "Pan left"),
        b(C::Graph, K::Right, NONE, A::Right, true, "→", "Pan right"),
        b(C::Graph, K::Char('k'), NONE, A::Up, false, "k", "Pan up"),
        b(
            C::Graph,
            K::Char('j'),
            NONE,
            A::Down,
            false,
            "j",
            "Pan down",
        ),
        b(
            C::Graph,
            K::Char('h'),
            NONE,
            A::Left,
            false,
            "h",
            "Pan left",
        ),
        b(
            C::Graph,
            K::Char('l'),
            NONE,
            A::Right,
            false,
            "l",
            "Pan right",
        ),
        b(
            C::Graph,
            K::Char('+'),
            NONE,
            A::ZoomIn,
            true,
            "+",
            "Zoom in",
        ),
        b(
            C::Graph,
            K::Char('='),
            NONE,
            A::ZoomIn,
            false,
            "=",
            "Zoom in",
        ),
        b(
            C::Graph,
            K::Char('-'),
            NONE,
            A::ZoomOut,
            true,
            "-",
            "Zoom out",
        ),
        b(
            C::Graph,
            K::Tab,
            NONE,
            A::NextNode,
            true,
            "Tab",
            "Next node",
        ),
        b(
            C::Graph,
            K::Char('n'),
            NONE,
            A::NextNode,
            false,
            "n",
            "Next node",
        ),
        b(
            C::Graph,
            K::BackTab,
            NONE,
            A::PrevNode,
            true,
            "S-Tab",
            "Previous node",
        ),
        b(
            C::Graph,
            K::Char('p'),
            NONE,
            A::PrevNode,
            false,
            "p",
            "Previous node",
        ),
        b(
            C::Graph,
            K::Enter,
            NONE,
            A::Activate,
            true,
            "⏎",
            "Open node in Explorer",
        ),
        b(
            C::Graph,
            K::Char('f'),
            NONE,
            A::FocusMode,
            true,
            "f",
            "Focus mode (k-hop)",
        ),
        b(
            C::Graph,
            K::Char('c'),
            NONE,
            A::CycleColor,
            true,
            "c",
            "Cycle node coloring",
        ),
        b(
            C::Graph,
            K::Char('s'),
            NONE,
            A::ToggleSources,
            true,
            "s",
            "Toggle source nodes",
        ),
        b(
            C::Graph,
            K::Char('b'),
            NONE,
            A::ToggleBroken,
            true,
            "b",
            "Toggle broken targets",
        ),
        b(
            C::Graph,
            K::Char('d'),
            NONE,
            A::ToggleDerivations,
            true,
            "d",
            "Toggle derivation edges",
        ),
        b(
            C::Graph,
            K::Char(' '),
            NONE,
            A::PauseLayout,
            true,
            "Space",
            "Pause / resume layout",
        ),
        b(
            C::Graph,
            K::Char('/'),
            NONE,
            A::GraphFilter,
            true,
            "/",
            "Filter nodes",
        ),
        b(
            C::Graph,
            K::Char('L'),
            NONE,
            A::CycleLayout,
            true,
            "L",
            "Cycle layout",
        ),
        // Mission control.
        b(C::Trust, K::Up, NONE, A::Up, true, "↑", "Previous row"),
        b(C::Trust, K::Down, NONE, A::Down, true, "↓", "Next row"),
        b(
            C::Trust,
            K::Char('k'),
            NONE,
            A::Up,
            false,
            "k",
            "Previous row",
        ),
        b(
            C::Trust,
            K::Char('j'),
            NONE,
            A::Down,
            false,
            "j",
            "Next row",
        ),
        b(
            C::Trust,
            K::Enter,
            NONE,
            A::Activate,
            true,
            "⏎",
            "Open / filter",
        ),
        b(
            C::Trust,
            K::Tab,
            NONE,
            A::CyclePanel,
            true,
            "Tab",
            "Cycle panel focus",
        ),
        b(
            C::Trust,
            K::Char('s'),
            NONE,
            A::CycleSort,
            true,
            "s",
            "Cycle queue sort",
        ),
        b(
            C::Trust,
            K::Char('d'),
            NONE,
            A::Remove,
            false,
            "d",
            "Remove concept",
        ),
        b(C::Trust, K::Home, NONE, A::Home, true, "Home", "First row"),
        b(C::Trust, K::End, NONE, A::End, true, "End", "Last row"),
        // Computations.
        b(C::Computations, K::Up, NONE, A::Up, true, "↑", "Previous"),
        b(C::Computations, K::Down, NONE, A::Down, true, "↓", "Next"),
        b(
            C::Computations,
            K::Char('k'),
            NONE,
            A::Up,
            false,
            "k",
            "Previous",
        ),
        b(
            C::Computations,
            K::Char('j'),
            NONE,
            A::Down,
            false,
            "j",
            "Next",
        ),
        b(
            C::Computations,
            K::Tab,
            NONE,
            A::CyclePanel,
            true,
            "Tab",
            "Focus contracts / playground",
        ),
        b(
            C::Computations,
            K::Enter,
            NONE,
            A::Activate,
            true,
            "⏎",
            "Open in Explorer",
        ),
        b(
            C::Computations,
            K::Char('y'),
            CTRL,
            A::CopySketch,
            true,
            "Ctrl+Y",
            "Copy invocation sketch",
        ),
    ]
};

/// Whether a key event matches a binding. `SHIFT` is ignored for `Char`
/// codes, since the char itself already encodes the case.
fn matches(binding: &Binding, key: &KeyEvent) -> bool {
    if binding.code != key.code {
        return false;
    }
    let ignore = if matches!(key.code, KeyCode::Char(_)) {
        KeyModifiers::SHIFT
    } else {
        KeyModifiers::NONE
    };
    (key.modifiers - ignore) == binding.mods
}

/// Looks up an action in exactly one context.
#[must_use]
pub fn lookup(context: Context, key: &KeyEvent) -> Option<Action> {
    bindings()
        .iter()
        .find(|binding| binding.context == context && matches(binding, key))
        .map(|binding| binding.action)
}

/// Resolves a key against a context chain, most specific first.
#[must_use]
pub fn resolve(contexts: &[Context], key: &KeyEvent) -> Option<Action> {
    contexts.iter().find_map(|&c| lookup(c, key))
}

/// The primary bindings for a context, in table order — the hint bar's feed.
pub fn hints(context: Context) -> impl Iterator<Item = &'static Binding> {
    bindings()
        .iter()
        .filter(move |binding| binding.context == context && binding.primary)
}
