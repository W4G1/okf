//! The studio's visual vocabulary: one glyph-and-color encoding per semantic
//! dimension, used identically in every workspace.
//!
//! Color is never the only channel: every colored state has a glyph, so the
//! UI degrades gracefully on monochrome terminals. The palette is drawn from
//! the terminal's 16 ANSI colors so the user's scheme is respected, and
//! `NO_COLOR` disables color entirely.

use okf_core::{ActorKind, Status, TrustTier};
use ratatui::style::{Color, Modifier, Style};

/// Glyph for a plain concept node.
pub const GLYPH_CONCEPT: &str = "●";
/// Glyph for an Attested Computation concept.
pub const GLYPH_COMPUTATION: &str = "⚙";
/// Glyph for a reserved `index.md` file.
pub const GLYPH_INDEX: &str = "◈";
/// Glyph for a reserved `log.md` file.
pub const GLYPH_LOG: &str = "≡";
/// Glyph for a parse error or broken target.
pub const GLYPH_BROKEN: &str = "✗";
/// Suffix glyph for a stale concept or one carrying diagnostics.
pub const GLYPH_WARN: &str = "⚠";
/// Glyph for a concept going stale within 30 days.
pub const GLYPH_STALE_SOON: &str = "⏳";
/// Glyph for a fixable diagnostic.
pub const GLYPH_FIXABLE: &str = "✚";
/// Glyph for a passing check.
pub const GLYPH_OK: &str = "✔";

/// Braille spinner frames shown while a reload or apply is in flight.
pub const SPINNER_FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// The active theme: whether color is enabled, and the style vocabulary.
#[derive(Clone, Copy, Debug)]
pub struct Theme {
    /// `false` when `NO_COLOR` is set; every style collapses to monochrome.
    pub color: bool,
}

impl Default for Theme {
    fn default() -> Self {
        Self::from_env()
    }
}

impl Theme {
    /// Builds the theme from the environment, honoring `NO_COLOR`.
    #[must_use]
    pub fn from_env() -> Self {
        Self {
            color: std::env::var_os("NO_COLOR").is_none_or(|v| v.is_empty()),
        }
    }

    /// A theme with color forced on or off (used by tests).
    #[must_use]
    pub const fn with_color(color: bool) -> Self {
        Self { color }
    }

    fn fg(self, color: Color) -> Style {
        if self.color {
            Style::default().fg(color)
        } else {
            Style::default()
        }
    }

    /// The accent style for chrome highlights (tab bar, headings).
    #[must_use]
    pub fn accent(self) -> Style {
        self.fg(Color::Cyan)
    }

    /// The style for dimmed, secondary text.
    #[must_use]
    pub fn dim(self) -> Style {
        Style::default().add_modifier(Modifier::DIM)
    }

    /// The style for errors and broken targets.
    #[must_use]
    pub fn error(self) -> Style {
        self.fg(Color::Red)
    }

    /// The style for warnings and staleness.
    #[must_use]
    pub fn warn(self) -> Style {
        self.fg(Color::Yellow)
    }

    /// The style for success verdicts.
    #[must_use]
    pub fn ok(self) -> Style {
        self.fg(Color::Green)
    }

    /// The style for the selected row / focused element.
    #[must_use]
    pub fn selection(self) -> Style {
        Style::default().add_modifier(Modifier::REVERSED)
    }

    /// The primary hue for a trust tier, the dimension every workspace colors
    /// by first.
    #[must_use]
    pub fn tier(self, tier: TrustTier) -> Style {
        match tier {
            TrustTier::HumanReviewed => self.fg(Color::Green),
            TrustTier::MachineConfirmed => self.fg(Color::Blue),
            TrustTier::Unverified => self.fg(Color::DarkGray),
        }
    }

    /// The style for an actor string, keyed by its kind.
    #[must_use]
    pub fn actor(self, kind: ActorKind) -> Style {
        match kind {
            ActorKind::Human => self.fg(Color::Cyan),
            ActorKind::Process => self.fg(Color::Magenta),
            ActorKind::Agent => self.fg(Color::Blue),
            ActorKind::Other => Style::default(),
        }
    }

    /// The style for a lifecycle status.
    #[must_use]
    pub fn status(self, status: &Status) -> Style {
        match status {
            Status::Draft => self.fg(Color::Yellow),
            Status::Deprecated => self.dim().add_modifier(Modifier::CROSSED_OUT),
            Status::Stable | Status::Other(_) => Style::default(),
        }
    }
}

/// The glyph for a trust tier: `◆` human-reviewed, `●` machine-confirmed,
/// `○` unverified.
#[must_use]
pub const fn tier_glyph(tier: TrustTier) -> &'static str {
    match tier {
        TrustTier::HumanReviewed => "◆",
        TrustTier::MachineConfirmed => "●",
        TrustTier::Unverified => "○",
    }
}

/// The glyph for a lifecycle status: `●` stable, `◐` draft, `◌` deprecated.
#[must_use]
pub const fn status_glyph(status: &Status) -> &'static str {
    match status {
        Status::Draft => "◐",
        Status::Deprecated => "◌",
        Status::Stable | Status::Other(_) => "●",
    }
}
