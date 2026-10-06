//! The semantic roles the Styra UI colors by.
//!
//! Every entry here names a role, not a color: renderers select from this
//! module so the interface can be recolored in one place by editing
//! [`crate::palette`] instead. Where several roles share a hue, they point at
//! the same palette constant rather than repeating its value.

use crate::palette;
use ratatui::style::Color;

/// `Normal`.
pub const TEXT: Color = palette::WHITE;
/// `Visual` / `TabLine` text.
pub const MUTED_TEXT: Color = palette::LIGHT_GRAY;

/// Low-emphasis supporting information, such as ids, origins, queued text,
/// and explanatory suffixes.
pub const ADDITIONAL_INFO: Color = palette::TAN;

/// `Type`.
pub const ACCENT: Color = palette::OLIVE;
pub const LIGHT_ACCENT: Color = palette::DUSTY_ROSE;
/// `Function`.
pub const INFO: Color = palette::PALE_PINK;
pub const SUCCESS: Color = palette::TERMINAL_GREEN;
pub const WARNING: Color = palette::ROSE;
/// The running status in a pane's top border, in the hue of the log's own
/// `working …` line.
pub const RUNNING_STATUS: Color = RUNNING;
/// A quota nearing its limit, on the quota screen and in the footer.
pub const QUOTA_WARNING: Color = palette::TERMINAL_YELLOW;
/// A workspace heading in the live-interactions navigator.
pub const WORKSPACE_NAME: Color = palette::GOLD;
/// An interaction's `#tags`: apart from the status flags that share their
/// row.
pub const INTERACTION_TAG: Color = palette::PEACH;
/// Work left uncommitted: the `!` ahead of such an interaction and the
/// notice on its pane's border.
pub const UNCOMMITTED: Color = palette::AMBER_YELLOW;
/// The spinner of a running turn, in the navigator and the log: the loudest
/// hue on its row without glaring, and apart from the yellow that means work
/// left uncommitted.
pub const RUNNING: Color = UNCOMMITTED;
pub const ERROR: Color = palette::TERMINAL_RED;
/// `Special`.
pub const SPECIAL: Color = palette::PINK;
/// The warning hue a step down.
pub const MUTED_WARNING: Color = palette::MUTED_ROSE;
/// `SignColumn`.
pub const CODE_BACKGROUND: Color = palette::NEAR_BLACK;
/// Inline Markdown code that is neither a link nor a file reference, on the
/// terminal's own black, so a quoted term reads apart from the yellow of the
/// code that can be followed.
pub const INLINE_CODE: Color = palette::PEACH;
pub const INLINE_CODE_BACKGROUND: Color = palette::TERMINAL_BLACK;
/// Inline code that is also a link or a file reference, on orgone's
/// `DiffChange` red.
pub const ENTRY_CODE: Color = palette::TERMINAL_YELLOW;
pub const ENTRY_CODE_BACKGROUND: Color = palette::DARK_WINE;

/// Truly inactive controls and chrome retain a subdued, low-key color.
pub const INACTIVE: Color = palette::STONE;
pub const MODAL_BACKDROP: Color = palette::STONE;

/// Reason text appended to a row in the live-interactions navigator.
///
/// Kept separate from inactive chrome so this status information can be
/// recolored without changing disabled controls, borders, or markers.
pub const INTERACTION_STATUS_INFO: Color = palette::STONE;

/// Why a stopped interaction stopped, one hue per reason, so the way it ended
/// reads at a glance in the status title, the navigator row and the log tail.
/// The operator paused it: the tan of supporting information, calm and
/// deliberate.
pub const STOP_PAUSED: Color = ADDITIONAL_INFO;
/// The operator finished with it.
pub const STOP_COMPLETED: Color = palette::GREEN;
/// The operator gave up on it.
pub const STOP_ABANDONED: Color = palette::SLATE;
/// Finished for good.
pub const STOP_SEALED: Color = palette::DEEP_GREEN;
/// A plan window refused the work.
pub const STOP_RATE_LIMITED: Color = palette::AMBER_ORANGE;
/// The last turn failed.
pub const STOP_FAILED: Color = ERROR;
/// The agent's process went away on its own.
pub const STOP_EXITED: Color = SPECIAL;
/// The server run that owned it went down.
pub const STOP_SERVER_RESTARTED: Color = palette::MAUVE;
/// Stopped without saying why.
pub const STOP_UNKNOWN: Color = INACTIVE;

/// A restrained cue behind operator-authored rows, separating prompts from
/// agent output without turning the log into chat bubbles. Paired with
/// [`USER_TEXT`].
pub const USER_MESSAGE_BACKGROUND: Color = palette::DARK_GREEN_TINT;
/// `CursorLine`.
pub const SELECTION_BACKGROUND: Color = palette::DARK_GRAY;
/// A slight lift behind an interaction's own line in the live-interactions
/// navigator, so it stands above the last-message line beneath it. Each
/// status carries a faint wash of its marker's hue, so the entries in one
/// state read as a group down the list.
///
/// Waiting to start: the pale rose of [`INFO`].
pub const PENDING_INTERACTION_ROW_BACKGROUND: Color = palette::DARK_ROSE_TINT;
/// Working: the amber of [`RUNNING`].
pub const RUNNING_INTERACTION_ROW_BACKGROUND: Color = palette::DARK_AMBER_TINT;
/// Idle: the green of [`SUCCESS`].
pub const IDLE_INTERACTION_ROW_BACKGROUND: Color = palette::DARK_GREEN_TINT_DEEP;
/// Working in the background: the dusty rose of [`MUTED_WARNING`].
pub const BACKGROUND_INTERACTION_ROW_BACKGROUND: Color = palette::MUTED_ROSE_TINT;
/// Stopped, waiting on a decision.
pub const STOPPED_INTERACTION_ROW_BACKGROUND: Color = palette::DARK_RUST_TINT;
/// Failed: the red of [`ERROR`].
pub const ERROR_INTERACTION_ROW_BACKGROUND: Color = palette::DARK_RED_TINT;
/// Ended: the stone of [`INACTIVE`].
pub const ENDED_INTERACTION_ROW_BACKGROUND: Color = palette::DARK_STONE_TINT;
/// The navigator's cursor row, a step above [`SELECTION_BACKGROUND`] so it
/// stands clear of the lifted rows around it.
pub const INTERACTION_SELECTION_BACKGROUND: Color = palette::MEDIUM_GRAY;
/// Text on a continuation line. It is subdued without looking disabled.
pub const SUBORDINATE_TEXT: Color = MUTED_TEXT;
/// `PreProc` / `MatchParen` text.
pub const SELECTION_MARKER: Color = palette::BRIGHT_YELLOW;
/// The dot marking the Interaction currently open in the main view. This is
/// intentionally the terminal's bright yellow so it remains immediately
/// visible alongside the navigator cursor and status icons.
pub const CURRENT_INTERACTION_MARKER: Color = palette::TERMINAL_YELLOW;
/// The focused Markdown link: visible without the hard yellow used for a row
/// cursor, since it sits directly behind the link's own syntax styling.
pub const LINK_HIGHLIGHT_BACKGROUND: Color = palette::MUTED_WINE;
/// Markdown link labels: the rose of [`LIGHT_ACCENT`], so a link reads as a
/// link rather than as the near-white of [`INFO`].
pub const MARKDOWN_LINK: Color = LIGHT_ACCENT;
/// Markdown headings: the gold of a navigator group heading, so a section
/// in a reply reads like a section anywhere else in the UI.
pub const MARKDOWN_HEADING: Color = WORKSPACE_NAME;
/// Markdown block quotes: the tan of supporting information, in place of
/// the terminal's own green, which belongs to success and the operator.
pub const MARKDOWN_QUOTE: Color = ADDITIONAL_INFO;
pub const LIVE_MARKER: Color = SUCCESS;

pub const AGENT_TAG: Color = palette::TAN;
pub const USER_TAG: Color = palette::MEDIUM_GREEN;
/// `Directory`.
pub const SHELL_TAG: Color = palette::ORANGE;
pub const AGENT_TEXT: Color = TEXT;
/// On [`USER_MESSAGE_BACKGROUND`].
pub const USER_TEXT: Color = palette::PALE_GREEN;

/// `Identifier`.
pub const JSON_KEY: Color = palette::PALE_YELLOW;
pub const JSON_STRING: Color = palette::PEACH;
pub const JSON_NUMBER: Color = palette::PEACH;
pub const JSON_LITERAL: Color = SPECIAL;
pub const JSON_PUNCTUATION: Color = INACTIVE;

/// `tui-markdown` delegates fenced-code highlighting to a TextMate theme, so
/// its syntax colors live here too. TextMate requires literal RGB hex values;
/// keeping the theme beside the other entries preserves one configuration
/// point for the entire UI. The scopes mirror orgone's syntax groups and the
/// hex values match the [`crate::palette`] entries of the same hue.
pub const MARKDOWN_CODE_KEYWORD: Color = palette::TAN;
pub const MARKDOWN_CODE_THEME: &str = r##"
<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0">
<dict>
  <key>name</key><string>Styra</string>
  <key>settings</key>
  <array>
    <dict><key>settings</key><dict>
      <key>background</key><string>#121212</string>
      <key>foreground</key><string>#FFFFFF</string>
    </dict></dict>
    <dict><key>scope</key><string>comment</string><key>settings</key><dict>
      <key>foreground</key><string>#A8A8A8</string>
    </dict></dict>
    <dict><key>scope</key><string>string</string><key>settings</key><dict>
      <key>foreground</key><string>#FFD7AF</string>
    </dict></dict>
    <dict><key>scope</key><string>constant, constant.numeric</string><key>settings</key><dict>
      <key>foreground</key><string>#FFD7AF</string>
    </dict></dict>
    <dict><key>scope</key><string>keyword, storage</string><key>settings</key><dict>
      <key>foreground</key><string>#D7AF87</string>
    </dict></dict>
    <dict><key>scope</key><string>keyword.operator, constant.character.escape</string><key>settings</key><dict>
      <key>foreground</key><string>#FF87AF</string>
    </dict></dict>
    <dict><key>scope</key><string>meta.preprocessor, keyword.control.import</string><key>settings</key><dict>
      <key>foreground</key><string>#FFFF87</string>
    </dict></dict>
    <dict><key>scope</key><string>entity.name.function</string><key>settings</key><dict>
      <key>foreground</key><string>#FFD7D7</string>
    </dict></dict>
    <dict><key>scope</key><string>support.function, meta.function-call</string><key>settings</key><dict>
      <key>foreground</key><string>#D7B2BC</string>
    </dict></dict>
    <dict><key>scope</key><string>entity.name.type, storage.type, support.type</string><key>settings</key><dict>
      <key>foreground</key><string>#D7D787</string>
    </dict></dict>
    <dict><key>scope</key><string>variable, entity.name.tag</string><key>settings</key><dict>
      <key>foreground</key><string>#FFFFD7</string>
    </dict></dict>
    <dict><key>scope</key><string>invalid</string><key>settings</key><dict>
      <key>foreground</key><string>#AA0000</string>
    </dict></dict>
  </array>
</dict>
</plist>
"##;

pub const RESET: Color = palette::RESET;
