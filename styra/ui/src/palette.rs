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
/// A workspace heading in the live-interactions navigator: a soft gold, so a
/// group reads clearly without the glare of the terminal's own yellow.
pub const WORKSPACE_NAME: Color = Color::Rgb(230, 200, 120);
/// An interaction's `#tags`: orgone's `String` peach (223), apart from the
/// status flags that share their row.
pub const INTERACTION_TAG: Color = Color::Rgb(255, 215, 175);
/// Work left uncommitted: the `!` ahead of such an interaction and the
/// notice on its pane's border. A clear yellow (221), since it is the one
/// mark asking for action.
pub const UNCOMMITTED: Color = Color::Rgb(255, 215, 95);
/// The spinner of a running turn, in the navigator and the log: a bright
/// orange, the loudest hue on its row, and apart from the yellow that means
/// work left uncommitted.
pub const RUNNING: Color = Color::Rgb(255, 135, 0);
/// The terminal's own red, as before the orgone port.
pub const ERROR: Color = Color::Red;
/// 211, `Special`.
pub const SPECIAL: Color = Color::Rgb(255, 135, 175);
/// The warning hue a step down, kept bright enough to read.
pub const MUTED_WARNING: Color = Color::Rgb(170, 128, 128);
/// 233, `SignColumn`.
pub const CODE_BACKGROUND: Color = Color::Rgb(18, 18, 18);
/// Inline Markdown code: the terminal's own yellow on its own black, as
/// before the orgone port.
pub const INLINE_CODE: Color = Color::Yellow;
pub const INLINE_CODE_BACKGROUND: Color = Color::Black;
/// Inline code that is also a link or a file reference keeps the code's
/// yellow, on orgone's `DiffChange` red, so it reads apart from plain code.
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
/// Text on a continuation line. It is subdued without looking disabled.
pub const SUBORDINATE_TEXT: Color = MUTED_TEXT;
/// 228, `PreProc` / `MatchParen` text.
pub const SELECTION_MARKER: Color = Color::Rgb(255, 255, 135);
/// The focused Markdown link: visible without the hard yellow used for a row
/// cursor, since it sits directly behind the link's own syntax styling.
/// 101, `MatchParen`.
pub const LINK_HIGHLIGHT_BACKGROUND: Color = Color::Rgb(135, 135, 95);
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
