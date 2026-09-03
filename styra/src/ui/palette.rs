//! The complete Styra UI palette.
//!
//! Keep raw [`Color`] values in this module only. Renderers should select a
//! semantic entry from here so the interface can be recolored in one place.

use ratatui::style::Color;

pub(crate) const TEXT: Color = Color::White;
pub(crate) const MUTED_TEXT: Color = Color::Gray;

/// Low-emphasis supporting information, such as ids, origins, queued text,
/// and explanatory suffixes. The deliberately desaturated red replaces the
/// dark gray that previously made this information look disabled.
pub(crate) const ADDITIONAL_INFO: Color = Color::Rgb(211, 158, 96);

pub(crate) const ACCENT: Color = Color::Cyan;
pub(crate) const LIGHT_ACCENT: Color = Color::LightCyan;
pub(crate) const INFO: Color = Color::Blue;
pub(crate) const SUCCESS: Color = Color::Green;
pub(crate) const WARNING: Color = Color::Yellow;
pub(crate) const ERROR: Color = Color::Red;
pub(crate) const SPECIAL: Color = Color::Magenta;
pub(crate) const MUTED_WARNING: Color = Color::LightYellow;
pub(crate) const CODE_BACKGROUND: Color = Color::Black;

/// Truly inactive controls and chrome retain neutral dark gray.
pub(crate) const INACTIVE: Color = Color::DarkGray;
pub(crate) const MODAL_BACKDROP: Color = Color::DarkGray;

pub(crate) const SELECTION_BACKGROUND: Color = Color::Rgb(44, 42, 30);
/// A slightly darker tint than the surrounding rows, for a continuation line
/// that belongs to the row above it rather than standing on its own.
pub(crate) const SUBORDINATE_BACKGROUND: Color = Color::Rgb(24, 24, 24);
/// Text on a continuation line. It is subdued without looking disabled.
pub(crate) const SUBORDINATE_TEXT: Color = Color::Rgb(190, 190, 150);
pub(crate) const SELECTION_MARKER: Color = Color::Yellow;
pub(crate) const LIVE_MARKER: Color = Color::Green;

pub(crate) const AGENT_TAG: Color = Color::Rgb(211, 158, 96);
pub(crate) const USER_TAG: Color = Color::Rgb(115, 190, 137);
pub(crate) const SHELL_TAG: Color = Color::Rgb(184, 124, 0);
// pub(crate) const AGENT_TEXT: Color = Color::Rgb(238, 219, 193);
pub(crate) const AGENT_TEXT: Color = Color::White;
pub(crate) const USER_TEXT: Color = Color::Rgb(207, 243, 214);

pub(crate) const JSON_KEY: Color = Color::Cyan;
pub(crate) const JSON_STRING: Color = Color::Green;
pub(crate) const JSON_NUMBER: Color = Color::Rgb(184, 124, 0);
pub(crate) const JSON_LITERAL: Color = Color::Magenta;
pub(crate) const JSON_PUNCTUATION: Color = Color::DarkGray;

/// `tui-markdown` delegates fenced-code highlighting to a TextMate theme, so
/// its syntax colors live here too. TextMate requires literal RGB hex values;
/// keeping the theme beside the ratatui entries preserves one configuration
/// point for the entire UI.
#[cfg(test)]
pub(crate) const MARKDOWN_CODE_KEYWORD: Color = Color::Rgb(0, 255, 255);
pub(crate) const MARKDOWN_CODE_THEME: &str = r##"
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

#[cfg(test)]
pub(crate) const RESET: Color = Color::Reset;
