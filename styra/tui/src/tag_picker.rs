//! Modal editor for an interaction's durable tags, and the same list
//! choosing which tags the live-interactions navigator is narrowed to.
//!
//! The list of known tags narrows as it is typed at, through
//! [`styra_ui::fuzzy_list`], so every printable key is a letter of the query
//! and the picker's own commands mostly sit on control or navigation keys.
//! In filter mode, `-` is reserved for excluding the highlighted tag.

use styra_ui::fuzzy_list::FuzzyList;
use styra_ui::query::Edit;

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
    initial_selected: Vec<String>,
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
        if purpose == TagPurpose::Edit {
            for tag in &selected {
                if !available.contains(tag) {
                    available.push(tag.clone());
                }
            }
        }
        available.sort();
        Self {
            purpose,
            list: FuzzyList::at(&available, 0),
            available,
            initial_selected: selected.clone(),
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

    /// Make a change typed into the filter, as every search field takes it
    /// (see [`styra_ui::query`]).
    pub fn type_query(&mut self, edit: Edit) {
        self.list.edit(edit);
    }

    pub fn clear_query(&mut self) {
        self.list.clear(&self.available);
    }

    pub fn restore_selection(&mut self) {
        self.selected.clone_from(&self.initial_selected);
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
        let excluded = format!("-{tag}");
        if let Some(index) = self.selected.iter().position(|selected| selected == &tag) {
            self.selected.remove(index);
        } else {
            self.selected.retain(|selected| selected != &excluded);
            self.selected.push(tag);
            self.selected.sort();
        }
        self.clear_query();
    }

    /// Toggle the highlighted tag as an exclusion. A tag cannot be both
    /// required and excluded; switching its sense replaces the old choice.
    pub fn toggle_excluded(&mut self) {
        if self.purpose != TagPurpose::Filter {
            return;
        }
        let Some(tag) = self
            .list
            .selected_row(&self.available)
            .map(|row| self.available[row].clone())
        else {
            return;
        };
        let excluded = format!("-{tag}");
        if let Some(index) = self
            .selected
            .iter()
            .position(|selected| selected == &excluded)
        {
            self.selected.remove(index);
        } else {
            self.selected.retain(|selected| selected != &tag);
            self.selected.push(excluded);
            self.selected.sort();
        }
        self.clear_query();
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
mod tests;
