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
/// A brighter, warmer yellow (221 proper), lit up against [`AMBER_YELLOW`].
pub const BRIGHT_AMBER: Color = Color::Rgb(255, 215, 95);
/// [`WHITE`] with a touch of yellow.
pub const YELLOW_WHITE: Color = Color::Rgb(255, 245, 205);
/// [`WHITE`] with a touch of green.
pub const GREEN_WHITE: Color = Color::Rgb(225, 250, 225);
/// A soft, light cyan: [`WHITE`] leaning well toward blue-green.
pub const SOFT_CYAN: Color = Color::Rgb(175, 225, 230);
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
/// Between [`NEAR_BLACK`] and [`DARK_GRAY`], closer to the former.
pub const FAINT_GRAY: Color = Color::Rgb(28, 28, 28);
/// [`AMBER_ORANGE`] an eighth of the way up from [`NEAR_BLACK`].
pub const FAINT_ORANGE: Color = Color::Rgb(43, 33, 25);
/// 236, `CursorLine`.
pub const DARK_GRAY: Color = Color::Rgb(48, 48, 48);
/// [`PALE_PINK`] a fifth of the way up from [`NEAR_BLACK`].
pub const PALE_PINK_TINT: Color = Color::Rgb(70, 61, 61);
/// [`AMBER_YELLOW`] a fifth of the way up from [`NEAR_BLACK`].
pub const AMBER_TINT: Color = Color::Rgb(65, 58, 35);
/// A green a fifth of the way up from [`NEAR_BLACK`].
pub const GREEN_TINT: Color = Color::Rgb(30, 60, 34);
/// [`MUTED_ROSE`] a fifth of the way up from [`NEAR_BLACK`].
pub const MUTED_ROSE_TINT: Color = Color::Rgb(51, 42, 42);
/// [`STONE`] a fifth of the way up from [`NEAR_BLACK`].
pub const STONE_TINT: Color = Color::Rgb(53, 51, 47);
/// A red a fifth of the way up from [`NEAR_BLACK`].
pub const RED_TINT: Color = Color::Rgb(64, 24, 24);
pub const RED_FG: Color = Color::Rgb(255, 177, 177);
/// [`PALE_PINK`] nearly half the way up from [`NEAR_BLACK`].
pub const PALE_PINK_TINT_BRIGHT: Color = Color::Rgb(125, 107, 107);
/// [`AMBER_YELLOW`] nearly half the way up from [`NEAR_BLACK`].
pub const AMBER_TINT_BRIGHT: Color = Color::Rgb(113, 100, 53);
/// A green nearly half the way up from [`NEAR_BLACK`].
pub const GREEN_TINT_BRIGHT: Color = Color::Rgb(45, 110, 52);
/// [`MUTED_ROSE`] nearly half the way up from [`NEAR_BLACK`].
pub const MUTED_ROSE_TINT_BRIGHT: Color = Color::Rgb(86, 68, 68);
/// [`STONE`] nearly half the way up from [`NEAR_BLACK`].
pub const STONE_TINT_BRIGHT: Color = Color::Rgb(90, 86, 77);
/// A red nearly half the way up from [`NEAR_BLACK`].
pub const RED_TINT_BRIGHT: Color = Color::Rgb(120, 36, 36);
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
