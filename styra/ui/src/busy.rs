//! Progress for a synchronous call, displayed in the shared bottom status box.

/// The work in progress, in the operator's terms.
pub struct BusyView<'a> {
    pub message: &'a str,
}
