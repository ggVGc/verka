//! Modal editor for an interaction's durable tags.

#[derive(Clone, Debug)]
pub struct TagPicker {
    pub available: Vec<String>,
    pub selected: Vec<String>,
    pub cursor: usize,
    pub new_tag: Option<String>,
}

impl TagPicker {
    pub fn new(mut available: Vec<String>, mut selected: Vec<String>) -> Self {
        available.sort();
        available.dedup();
        selected.sort();
        selected.dedup();
        for tag in &selected {
            if !available.contains(tag) {
                available.push(tag.clone());
            }
        }
        available.sort();
        Self {
            available,
            selected,
            cursor: 0,
            new_tag: None,
        }
    }

    pub fn next(&mut self) {
        if !self.available.is_empty() {
            self.cursor = (self.cursor + 1) % self.available.len();
        }
    }

    pub fn previous(&mut self) {
        if !self.available.is_empty() {
            self.cursor = self
                .cursor
                .checked_sub(1)
                .unwrap_or(self.available.len() - 1);
        }
    }

    pub fn toggle(&mut self) {
        let Some(tag) = self.available.get(self.cursor).cloned() else {
            return;
        };
        if let Some(index) = self.selected.iter().position(|selected| selected == &tag) {
            self.selected.remove(index);
        } else {
            self.selected.push(tag);
            self.selected.sort();
        }
    }

    pub fn add_new(&mut self) {
        let Some(tag) = self
            .new_tag
            .take()
            .map(|tag| tag.trim().to_owned())
            .filter(|tag| !tag.is_empty())
        else {
            return;
        };
        if !self.available.contains(&tag) {
            self.available.push(tag.clone());
            self.available.sort();
        }
        if !self.selected.contains(&tag) {
            self.selected.push(tag);
            self.selected.sort();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::TagPicker;

    #[test]
    fn adding_a_tag_selects_it_and_makes_it_available() {
        let mut picker = TagPicker::new(vec!["bug".into()], vec![]);
        picker.new_tag = Some("urgent".into());
        picker.add_new();
        assert_eq!(picker.available, ["bug", "urgent"]);
        assert_eq!(picker.selected, ["urgent"]);
    }
}
