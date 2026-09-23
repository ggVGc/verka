//! The complete Styra UI palette.
//!
//! Keep raw [`Color`] values in this module only. Renderers should select a
//! semantic entry from here so the interface can be recolored in one place.

use ratatui::style::Color;

pub const TEXT: Color = Color::White;
pub const MUTED_TEXT: Color = Color::Gray;

/// Low-emphasis supporting information, such as ids, origins, queued text,
/// and explanatory suffixes. The deliberately desaturated red replaces the
/// dark gray that previously made this information look disabled.
pub const ADDITIONAL_INFO: Color = Color::Rgb(211, 158, 96);

pub const ACCENT: Color = Color::Cyan;
pub const LIGHT_ACCENT: Color = Color::LightCyan;
pub const INFO: Color = Color::Rgb(250, 182, 179);
pub const SUCCESS: Color = Color::Green;
pub const WARNING: Color = Color::Yellow;
pub const ERROR: Color = Color::Red;
pub const SPECIAL: Color = Color::Magenta;
pub const MUTED_WARNING: Color = Color::LightYellow;
pub const CODE_BACKGROUND: Color = Color::Black;

/// `Color::DarkGray` renders too dark to read comfortably in most terminals;
/// this desaturated slate blue replaces it while staying subdued.
pub const MUTED_SLATE: Color = Color::Rgb(115, 130, 155);

/// Truly inactive controls and chrome retain a subdued, low-key color.
pub const INACTIVE: Color = MUTED_SLATE;
pub const MODAL_BACKDROP: Color = MUTED_SLATE;

pub const SELECTION_BACKGROUND: Color = Color::Rgb(44, 42, 30);
/// A slightly darker tint than the surrounding rows, for a continuation line
/// that belongs to the row above it rather than standing on its own.
pub const SUBORDINATE_BACKGROUND: Color = Color::Rgb(24, 24, 24);
/// Text on a continuation line. It is subdued without looking disabled.
pub const SUBORDINATE_TEXT: Color = Color::Rgb(190, 190, 150);
pub const SELECTION_MARKER: Color = Color::Yellow;
pub const LIVE_MARKER: Color = Color::Green;

pub const AGENT_TAG: Color = Color::Rgb(211, 158, 96);
pub const USER_TAG: Color = Color::Rgb(115, 190, 137);
pub const SHELL_TAG: Color = Color::Rgb(184, 124, 0);
// pub(crate) const AGENT_TEXT: Color = Color::Rgb(238, 219, 193);
pub const AGENT_TEXT: Color = Color::White;
pub const USER_TEXT: Color = Color::Rgb(207, 243, 214);

pub const JSON_KEY: Color = Color::Cyan;
pub const JSON_STRING: Color = Color::Green;
pub const JSON_NUMBER: Color = Color::Rgb(184, 124, 0);
pub const JSON_LITERAL: Color = Color::Magenta;
pub const JSON_PUNCTUATION: Color = MUTED_SLATE;

/// `tui-markdown` delegates fenced-code highlighting to a TextMate theme, so
/// its syntax colors live here too. TextMate requires literal RGB hex values;
/// keeping the theme beside the ratatui entries preserves one configuration
/// point for the entire UI.
pub const MARKDOWN_CODE_KEYWORD: Color = Color::Rgb(0, 255, 255);
pub const MARKDOWN_CODE_THEME: &str = r##"
<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0">
<dict>
  <key>name</key><string>Styra</string>
  <key>settings</key>
  <array>
    <dict><key>settings</key><dict>
      <key>background</key><string>#000000</string>
      <key>foreground</key><string>#FFFFFF</string>
    </dict></dict>
    <dict><key>scope</key><string>comment</string><key>settings</key><dict>
      <key>foreground</key><string>#704848</string>
    </dict></dict>
    <dict><key>scope</key><string>string</string><key>settings</key><dict>
      <key>foreground</key><string>#73BE89</string>
    </dict></dict>
    <dict><key>scope</key><string>constant, constant.numeric</string><key>settings</key><dict>
      <key>foreground</key><string>#B87C00</string>
    </dict></dict>
    <dict><key>scope</key><string>keyword, storage</string><key>settings</key><dict>
      <key>foreground</key><string>#00FFFF</string>
    </dict></dict>
    <dict><key>scope</key><string>entity.name.function, entity.name.type, support.function</string><key>settings</key><dict>
      <key>foreground</key><string>#FFFF00</string>
    </dict></dict>
    <dict><key>scope</key><string>variable, entity.name.tag</string><key>settings</key><dict>
      <key>foreground</key><string>#D39E60</string>
    </dict></dict>
    <dict><key>scope</key><string>invalid</string><key>settings</key><dict>
      <key>foreground</key><string>#FF0000</string>
    </dict></dict>
  </array>
</dict>
</plist>
"##;

pub const RESET: Color = Color::Reset;
