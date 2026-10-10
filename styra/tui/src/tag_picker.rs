//! Modal editor for an interaction's durable tags, and the same list
//! choosing which tags the live-interactions navigator is narrowed to.
//!
//! The list of known tags narrows as it is typed at, through
//! [`styra_ui::fuzzy_list`], so every printable key is a letter of the query
//! and the picker's own commands sit on keys no tag name contains.

use styra_ui::fuzzy_list::FuzzyList;

/// What the chosen tags are for, which decides what Enter does with them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TagPurpose {
    /// The tags of the current interaction, saved on Enter.
    Edit,
    /// The tags the navigator lists; a filter chooses among the tags that
    /// exist, so no new one can be added.
    Filter,
}

#[derive(Clone, Debug)]
pub struct TagPicker {
    pub purpose: TagPurpose,
    pub available: Vec<String>,
    pub selected: Vec<String>,
    pub list: FuzzyList,
    pub new_tag: Option<String>,
}

impl TagPicker {
    pub fn new(available: Vec<String>, selected: Vec<String>) -> Self {
        Self::for_purpose(TagPurpose::Edit, available, selected)
    }

    pub fn filter(available: Vec<String>, selected: Vec<String>) -> Self {
        Self::for_purpose(TagPurpose::Filter, available, selected)
    }

    fn for_purpose(
        purpose: TagPurpose,
        mut available: Vec<String>,
        mut selected: Vec<String>,
    ) -> Self {
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
            purpose,
            list: FuzzyList::at(&available, 0),
            available,
            selected,
            new_tag: None,
        }
    }

    pub fn next(&mut self) {
        self.list.next(&self.available);
    }

    pub fn previous(&mut self) {
        self.list.prev(&self.available);
    }

    pub fn page_down(&mut self) {
        self.list.page_down(&self.available);
    }

    pub fn page_up(&mut self) {
        self.list.page_up(&self.available);
    }

    /// Take a typed character into the filter, or drop the last one.
    pub fn type_query(&mut self, character: Option<char>) {
        match character {
            Some(character) => self.list.push(character),
            None => self.list.backspace(),
        }
    }

    pub fn delete_query_word(&mut self) {
        self.list.delete_word();
    }

    pub fn is_filtering(&self) -> bool {
        self.list.is_filtering()
    }

    pub fn clear_query(&mut self) {
        self.list.clear(&self.available);
    }

    /// Start typing a new tag, seeded with the filter: a query that matched
    /// nothing is most often the name of the tag that is missing.
    pub fn start_new(&mut self) {
        if self.purpose == TagPurpose::Filter {
            return;
        }
        self.new_tag = Some(self.list.query.trim().to_owned());
    }

    pub fn toggle(&mut self) {
        let Some(tag) = self
            .list
            .selected_row(&self.available)
            .map(|row| self.available[row].clone())
        else {
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
    fn a_filter_cannot_start_a_new_tag() {
        let mut picker = TagPicker::filter(vec!["bug".into()], vec![]);
        picker.type_query(Some('x'));
        picker.start_new();
        assert!(picker.new_tag.is_none());
    }

    #[test]
    fn adding_a_tag_selects_it_and_makes_it_available() {
        let mut picker = TagPicker::new(vec!["bug".into()], vec![]);
        picker.new_tag = Some("urgent".into());
        picker.add_new();
        assert_eq!(picker.available, ["bug", "urgent"]);
        assert_eq!(picker.selected, ["urgent"]);
    }

    #[test]
    fn typing_narrows_the_list_and_toggle_acts_on_the_match() {
        let mut picker = TagPicker::new(
            vec!["bug".into(), "feature".into(), "urgent".into()],
            vec![],
        );
        for character in "urg".chars() {
            picker.type_query(Some(character));
        }
        picker.toggle();
        assert_eq!(picker.selected, ["urgent"]);
    }

    #[test]
    fn toggling_with_nothing_matching_changes_nothing() {
        let mut picker = TagPicker::new(vec!["bug".into()], vec![]);
        for character in "zzz".chars() {
            picker.type_query(Some(character));
        }
        picker.toggle();
        assert!(picker.selected.is_empty());
    }

    #[test]
    fn a_new_tag_starts_from_the_filter() {
        let mut picker = TagPicker::new(vec!["bug".into()], vec![]);
        for character in "docs".chars() {
            picker.type_query(Some(character));
        }
        picker.start_new();
        assert_eq!(picker.new_tag.as_deref(), Some("docs"));
    }
}
