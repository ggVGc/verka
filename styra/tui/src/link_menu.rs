//! The menu of things to do with the link selected in link navigation.

/// Something that can be done with the selected link.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkAction {
    /// What Enter does: a file in the editor, a web address in the browser.
    Open,
    /// Add the file to this interaction's own Driva mounts, as the message
    /// editor's path prompt grants a path it names.
    Mount { writable: bool },
}

impl LinkAction {
    fn label(self, web: bool) -> &'static str {
        match self {
            Self::Open if web => "open in browser",
            Self::Open => "open in editor",
            Self::Mount { writable: false } => "mount read-only for this interaction",
            Self::Mount { writable: true } => "mount read-write for this interaction",
        }
    }
}

/// The open link menu. The destination is fixed when the menu opens, so its
/// title names the link the action will be taken on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkMenu {
    destination: String,
    web: bool,
    actions: Vec<LinkAction>,
    selected: usize,
}

impl LinkMenu {
    /// A web address names no file, so only a file link can be mounted.
    pub fn new(destination: String, web: bool) -> Self {
        let mut actions = vec![LinkAction::Open];
        if !web {
            actions.push(LinkAction::Mount { writable: false });
            actions.push(LinkAction::Mount { writable: true });
        }
        Self {
            destination,
            web,
            actions,
            selected: 0,
        }
    }

    pub fn destination(&self) -> &str {
        &self.destination
    }

    pub fn labels(&self) -> Vec<&'static str> {
        self.actions
            .iter()
            .map(|action| action.label(self.web))
            .collect()
    }

    pub fn selected_index(&self) -> usize {
        self.selected
    }

    pub fn selected(&self) -> LinkAction {
        self.actions[self.selected]
    }

    pub fn select_next(&mut self) {
        self.selected = (self.selected + 1) % self.actions.len();
    }

    pub fn select_previous(&mut self) {
        self.selected = (self.selected + self.actions.len() - 1) % self.actions.len();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_file_link_offers_to_be_mounted() {
        assert_eq!(
            LinkMenu::new("src/lib.rs".into(), false).labels(),
            [
                "open in editor",
                "mount read-only for this interaction",
                "mount read-write for this interaction",
            ]
        );
        assert_eq!(
            LinkMenu::new("https://example.com".into(), true).labels(),
            ["open in browser"]
        );
    }

    #[test]
    fn selection_wraps_within_the_actions() {
        let mut menu = LinkMenu::new("src/lib.rs".into(), false);
        menu.select_previous();
        assert_eq!(menu.selected(), LinkAction::Mount { writable: true });
        menu.select_next();
        assert_eq!(menu.selected(), LinkAction::Open);
    }
}
