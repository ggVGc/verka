use crate::log::LogLevel;

pub struct MessageView<'a> {
    pub text: &'a str,
    pub level: LogLevel,
}

#[cfg(feature = "renderer")]
pub use rendering::*;

#[cfg(feature = "renderer")]
#[path = "render/messages.rs"]
mod rendering;
