pub enum HelpRow<'a> {
    Section(&'a str),
    /// `keys` is owned where the caller builds it from a binding's own keys,
    /// which is why it is a [`Cow`] rather than a borrow.
    Binding {
        keys: std::borrow::Cow<'a, str>,
        action: &'a str,
    },
    Blank,
}

#[cfg(feature = "renderer")]
pub use rendering::*;

#[cfg(feature = "renderer")]
#[path = "render/help.rs"]
mod rendering;
