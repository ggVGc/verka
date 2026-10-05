//! The complete Styra UI palette.
//!
//! Keep raw [`Color`] values in this module only. Renderers should select a
//! semantic entry from here so the interface can be recolored in one place.
//!
//! The values follow the `orgone` Vim colorscheme: soft warm yellows, peaches,
//! pinks and mauves on a neutral dark gray scale. Each entry notes the xterm
//! 256 index and the orgone highlight group it is taken from.

use ratatui::style::Color;

/// 15, `Normal`.
pub const TEXT: Color = Color::Rgb(255, 255, 255);
/// 251, `Visual` / `TabLine` text.
pub const MUTED_TEXT: Color = Color::Rgb(198, 198, 198);

/// Low-emphasis supporting information, such as ids, origins, queued text,
/// and explanatory suffixes. 180, `Statement`: a soft tan that stays readable
/// without looking disabled.
pub const ADDITIONAL_INFO: Color = Color::Rgb(215, 175, 135);

/// 186, `Type`.
pub const ACCENT: Color = Color::Rgb(215, 215, 135);
/// Orgone's `@function.call` (182) warmed from lilac toward rose, keeping
/// blue out of the palette.
pub const LIGHT_ACCENT: Color = Color::Rgb(215, 178, 188);
/// 224, `Function`.
pub const INFO: Color = Color::Rgb(255, 215, 215);
/// The terminal's own green, as before the orgone port.
pub const SUCCESS: Color = Color::Green;
/// A dusty rose: orgone's `Todo` (139) warmed away from purple. Orgone's own
/// `WarningMsg` (170) is too loud for the amount of text drawn in this color.
pub const WARNING: Color = Color::Rgb(230, 168, 165);
/// The running status in a pane's top border, in the hue of the log's own
/// `working …` line.
pub const RUNNING_STATUS: Color = RUNNING;
/// A quota nearing its limit, on the quota screen and in the footer: the
/// terminal's own yellow, as before the orgone port.
pub const QUOTA_WARNING: Color = Color::Yellow;
/// A workspace heading in the live-interactions navigator: a soft gold, so a
/// group reads clearly without the glare of the terminal's own yellow.
pub const WORKSPACE_NAME: Color = Color::Rgb(230, 200, 120);
/// An interaction's `#tags`: orgone's `String` peach (223), apart from the
/// status flags that share their row.
pub const INTERACTION_TAG: Color = Color::Rgb(255, 215, 175);
/// Work left uncommitted: the `!` ahead of such an interaction and the
/// notice on its pane's border. A clear yellow (221), since it is the one
/// mark asking for action.
pub const UNCOMMITTED: Color = Color::Rgb(230, 200, 95);
/// The spinner of a running turn, in the navigator and the log: a soft
/// coral orange (209), the loudest hue on its row without glaring, and apart
/// from the yellow that means work left uncommitted.
pub const RUNNING: Color = UNCOMMITTED;
/// The terminal's own red, as before the orgone port.
pub const ERROR: Color = Color::Red;
/// 211, `Special`.
pub const SPECIAL: Color = Color::Rgb(255, 135, 175);
/// The warning hue a step down, kept bright enough to read.
pub const MUTED_WARNING: Color = Color::Rgb(170, 128, 128);
/// 233, `SignColumn`.
pub const CODE_BACKGROUND: Color = Color::Rgb(18, 18, 18);
/// Inline Markdown code that is neither a link nor a file reference: orgone's
/// `String` peach (223), on the terminal's own black, so a quoted term reads
/// apart from the yellow of the code that can be followed.
pub const INLINE_CODE: Color = Color::Rgb(255, 215, 175);
pub const INLINE_CODE_BACKGROUND: Color = Color::Black;
/// Inline code that is also a link or a file reference: the terminal's own
/// yellow, as before the orgone port, on orgone's `DiffChange` red.
pub const ENTRY_CODE: Color = Color::Yellow;
pub const ENTRY_CODE_BACKGROUND: Color = Color::Rgb(57, 31, 37);

/// A warm stone gray. `Color::DarkGray` renders too dark to read comfortably
/// in most terminals, and orgone's slate-blue `Folded` (103) both reads poorly
/// and brings in a blue the palette otherwise avoids; this stays subdued
/// while remaining legible.
pub const MUTED_STONE: Color = Color::Rgb(178, 168, 150);

/// Truly inactive controls and chrome retain a subdued, low-key color.
pub const INACTIVE: Color = MUTED_STONE;
pub const MODAL_BACKDROP: Color = MUTED_STONE;

/// Reason text appended to a row in the live-interactions navigator.
///
/// Kept separate from inactive chrome so this status information can be
/// recolored without changing disabled controls, borders, or markers.
pub const INTERACTION_STATUS_INFO: Color = MUTED_STONE;

/// Why a stopped interaction stopped, one hue per reason, so the way it ended
/// reads at a glance in the status title, the navigator row and the log tail.
/// The operator paused it: the tan of supporting information, calm and
/// deliberate.
pub const STOP_PAUSED: Color = ADDITIONAL_INFO;
/// The operator finished with it: a soft green, apart from the terminal green
/// of a live, idle agent.
pub const STOP_COMPLETED: Color = Color::Rgb(135, 200, 140);
/// Finished for good: the completed green a step deeper.
pub const STOP_SEALED: Color = Color::Rgb(95, 165, 110);
/// A plan window refused the work: an amber orange, waiting rather than
/// broken.
pub const STOP_RATE_LIMITED: Color = Color::Rgb(230, 145, 80);
/// The last turn failed.
pub const STOP_FAILED: Color = ERROR;
/// The agent's process went away on its own. 211, `Special`.
pub const STOP_EXITED: Color = SPECIAL;
/// The server run that owned it went down: a mauve, the one hue in the
/// palette that says "not anything the agent did".
pub const STOP_SERVER_RESTARTED: Color = Color::Rgb(205, 160, 215);
/// Stopped without saying why.
pub const STOP_UNKNOWN: Color = INACTIVE;

/// A restrained cue behind operator-authored rows, separating prompts from
/// agent output without turning the log into chat bubbles. Paired with
/// [`USER_TEXT`]; both predate the orgone port.
pub const USER_MESSAGE_BACKGROUND: Color = Color::Rgb(18, 28, 21);
/// 236, `CursorLine`.
pub const SELECTION_BACKGROUND: Color = Color::Rgb(48, 48, 48);
/// A slight lift behind an interaction's own line in the live-interactions
/// navigator, so it stands above the last-message line beneath it.
/// 234, `Pmenu`.
pub const INTERACTION_ROW_BACKGROUND: Color = Color::Rgb(28, 28, 28);
/// The same lift with a faint rose in it, behind a stopped interaction, so
/// the entries waiting on a decision read as a group down the list.
pub const STOPPED_INTERACTION_ROW_BACKGROUND: Color = Color::Rgb(40, 27, 30);
/// The navigator's cursor row, a step above [`SELECTION_BACKGROUND`] so it
/// stands clear of the lifted rows around it. 238.
pub const INTERACTION_SELECTION_BACKGROUND: Color = Color::Rgb(68, 68, 68);
/// Text on a continuation line. It is subdued without looking disabled.
pub const SUBORDINATE_TEXT: Color = MUTED_TEXT;
/// 228, `PreProc` / `MatchParen` text.
pub const SELECTION_MARKER: Color = Color::Rgb(255, 255, 135);
/// The dot marking the Interaction currently open in the main view. This is
/// intentionally the terminal's bright yellow so it remains immediately
/// visible alongside the navigator cursor and status icons.
pub const CURRENT_INTERACTION_MARKER: Color = Color::Yellow;
/// The focused Markdown link: visible without the hard yellow used for a row
/// cursor, since it sits directly behind the link's own syntax styling. A
/// dusty rose (95) under the rose of the link itself, in place of orgone's
/// olive `MatchParen`, which read as the one green-gray in a warm palette.
pub const LINK_HIGHLIGHT_BACKGROUND: Color = Color::Rgb(135, 95, 95);
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

/// 180, `Statement`.
pub const AGENT_TAG: Color = Color::Rgb(215, 175, 135);
pub const USER_TAG: Color = Color::Rgb(115, 190, 137);
/// 173, `Directory`.
pub const SHELL_TAG: Color = Color::Rgb(215, 135, 95);
pub const AGENT_TEXT: Color = TEXT;
/// A pale green, on [`USER_MESSAGE_BACKGROUND`].
pub const USER_TEXT: Color = Color::Rgb(207, 243, 214);

/// 230, `Identifier`.
pub const JSON_KEY: Color = Color::Rgb(255, 255, 215);
/// 223, `String`.
pub const JSON_STRING: Color = Color::Rgb(255, 215, 175);
/// 223, `Number`; orgone deliberately shares one hue for strings and numbers.
pub const JSON_NUMBER: Color = Color::Rgb(255, 215, 175);
pub const JSON_LITERAL: Color = SPECIAL;
pub const JSON_PUNCTUATION: Color = MUTED_STONE;

/// `tui-markdown` delegates fenced-code highlighting to a TextMate theme, so
/// its syntax colors live here too. TextMate requires literal RGB hex values;
/// keeping the theme beside the ratatui entries preserves one configuration
/// point for the entire UI. The scopes mirror orgone's syntax groups.
/// 180, `Statement`.
pub const MARKDOWN_CODE_KEYWORD: Color = Color::Rgb(215, 175, 135);
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
      <key>foreground</key><string>#FF0000</string>
    </dict></dict>
  </array>
</dict>
</plist>
"##;

pub const RESET: Color = Color::Reset;
