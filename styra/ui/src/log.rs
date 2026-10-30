pub enum LogLevel {
    Info,
    Warn,
    Error,
}

pub struct LogEntryView<'a> {
    pub level: LogLevel,
    pub message: &'a str,
}

#[cfg(feature = "renderer")]
pub use rendering::*;

#[cfg(feature = "renderer")]
#[path = "render/log.rs"]
mod rendering;
