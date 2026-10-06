//! The complete Styra UI palette.
//!
//! Keep raw [`Color`] values in this module only, each named once no matter
//! how many semantic roles share it. [`crate::theme`] assigns those roles,
//! so the interface can be recolored in one place.
//!
//! The values follow the `orgone` Vim colorscheme: soft warm yellows, peaches,
//! pinks and mauves on a neutral dark gray scale. Each entry notes the xterm
//! 256 index and the orgone highlight group it is taken from.

use ratatui::style::Color;

/// 15, `Normal`.
pub const WHITE: Color = Color::Rgb(255, 255, 255);
/// 251, `Visual` / `TabLine` text.
pub const LIGHT_GRAY: Color = Color::Rgb(198, 198, 198);
/// 180, `Statement`. Shared by every role that wants this soft tan: supporting
/// information, the agent's own tag, and the fenced-code keyword scope.
pub const TAN: Color = Color::Rgb(215, 175, 135);
/// 186, `Type`.
pub const OLIVE: Color = Color::Rgb(215, 215, 135);
/// Orgone's `@function.call` (182) warmed from lilac toward rose, keeping
/// blue out of the palette.
pub const DUSTY_ROSE: Color = Color::Rgb(215, 178, 188);
/// 224, `Function`.
pub const PALE_PINK: Color = Color::Rgb(255, 215, 215);
/// A dusty rose: orgone's `Todo` (139) warmed away from purple. Orgone's own
/// `WarningMsg` (170) is too loud for the amount of text drawn in this color.
pub const ROSE: Color = Color::Rgb(230, 168, 165);
/// A soft gold, so a navigator group heading reads clearly without the glare
/// of the terminal's own yellow.
pub const GOLD: Color = Color::Rgb(230, 200, 120);
/// 223, `String`; orgone deliberately shares one hue for strings and numbers.
/// Shared by every role that wants this peach: an interaction's `#tags`,
/// inline code, and JSON strings and numbers.
pub const PEACH: Color = Color::Rgb(255, 215, 175);
/// A clear amber yellow (221), the one mark asking for action.
pub const AMBER_YELLOW: Color = Color::Rgb(230, 200, 95);
/// 211, `Special`.
pub const PINK: Color = Color::Rgb(255, 135, 175);
/// The warning hue a step down, kept bright enough to read.
pub const MUTED_ROSE: Color = Color::Rgb(170, 128, 128);
/// 233, `SignColumn`.
pub const NEAR_BLACK: Color = Color::Rgb(18, 18, 18);
/// Orgone's `DiffChange` red.
pub const DARK_WINE: Color = Color::Rgb(57, 31, 37);
/// A warm stone gray. `Color::DarkGray` renders too dark to read comfortably
/// in most terminals, and orgone's slate-blue `Folded` (103) both reads poorly
/// and brings in a blue the palette otherwise avoids; this stays subdued
/// while remaining legible.
pub const STONE: Color = Color::Rgb(178, 168, 150);
/// A soft green, apart from the terminal green of a live, idle agent.
pub const GREEN: Color = Color::Rgb(135, 200, 140);
/// A cool slate, finished with like the greens but without their sense of
/// success, and nothing like a failure's red.
pub const SLATE: Color = Color::Rgb(140, 155, 185);
/// The completed green a step deeper.
pub const DEEP_GREEN: Color = Color::Rgb(95, 165, 110);
/// An amber orange, waiting rather than broken.
pub const AMBER_ORANGE: Color = Color::Rgb(230, 145, 80);
/// A mauve, the one hue in the palette that says "not anything the agent
/// did".
pub const MAUVE: Color = Color::Rgb(205, 160, 215);
/// A restrained cue behind operator-authored rows, separating prompts from
/// agent output without turning the log into chat bubbles.
pub const DARK_GREEN_TINT: Color = Color::Rgb(18, 28, 21);
/// 236, `CursorLine`.
pub const DARK_GRAY: Color = Color::Rgb(48, 48, 48);
/// A faint wash of [`ROSE`] behind a pending interaction's row.
pub const DARK_ROSE_TINT: Color = Color::Rgb(36, 28, 28);
/// A faint wash of [`AMBER_YELLOW`] behind a running interaction's row.
pub const DARK_AMBER_TINT: Color = Color::Rgb(36, 32, 14);
/// A faint wash of green behind an idle interaction's row.
pub const DARK_GREEN_TINT_DEEP: Color = Color::Rgb(16, 34, 18);
/// A faint wash of [`MUTED_ROSE`] behind a backgrounded interaction's row.
pub const MUTED_ROSE_TINT: Color = Color::Rgb(34, 24, 24);
/// A faint rust behind a stopped interaction's row.
pub const DARK_RUST_TINT: Color = Color::Rgb(40, 20, 10);
/// A faint wash of red behind a failed interaction's row.
pub const DARK_RED_TINT: Color = Color::Rgb(44, 14, 14);
/// A faint wash of stone behind an ended interaction's row.
pub const DARK_STONE_TINT: Color = Color::Rgb(30, 29, 26);
/// 238.
pub const MEDIUM_GRAY: Color = Color::Rgb(68, 68, 68);
/// 228, `PreProc` / `MatchParen` text.
pub const BRIGHT_YELLOW: Color = Color::Rgb(255, 255, 135);
/// A dusty rose (95) under the rose of the link itself, in place of orgone's
/// olive `MatchParen`, which read as the one green-gray in a warm palette.
pub const MUTED_WINE: Color = Color::Rgb(135, 95, 95);
/// A soft green distinct from [`GREEN`], for the operator's own tag.
pub const MEDIUM_GREEN: Color = Color::Rgb(115, 190, 137);
/// 173, `Directory`.
pub const ORANGE: Color = Color::Rgb(215, 135, 95);
/// A pale green.
pub const PALE_GREEN: Color = Color::Rgb(207, 243, 214);
/// 230, `Identifier`.
pub const PALE_YELLOW: Color = Color::Rgb(255, 255, 215);

/// The terminal's own green, as before the orgone port.
pub const TERMINAL_GREEN: Color = Color::Green;
/// The terminal's own yellow, as before the orgone port.
pub const TERMINAL_YELLOW: Color = Color::Yellow;
/// The terminal's own red, as before the orgone port.
pub const TERMINAL_RED: Color = Color::Red;
/// The terminal's own black.
pub const TERMINAL_BLACK: Color = Color::Black;

pub const RESET: Color = Color::Reset;
