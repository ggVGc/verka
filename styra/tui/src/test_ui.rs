//! Application test double. Never invokes a renderer; feedback and input are explicit.
use std::{collections::VecDeque, time::Duration};
use styra_ui::{application::ApplicationView, *};

type ApplicationCallback<'a> = dyn FnMut(&ApplicationView<'_>) -> UiResult<RenderFeedback> + 'a;
pub(crate) struct TestUi<'a> {
    pub application: Box<ApplicationCallback<'a>>,
    pub applications: usize,
    events: VecDeque<crossterm::event::Event>,
}
impl Default for TestUi<'_> {
    fn default() -> Self {
        Self {
            application: Box::new(|_| Ok(RenderFeedback::default())),
            applications: 0,
            events: VecDeque::new(),
        }
    }
}
impl TestUi<'_> {
    pub fn script(&mut self, events: impl IntoIterator<Item = crossterm::event::Event>) {
        self.events.extend(events);
    }
}
#[allow(unused_variables, clippy::too_many_arguments)]
impl Ui for TestUi<'_> {
    fn render_application(
        &mut self,
        view: &application::ApplicationView<'_>,
    ) -> UiResult<RenderFeedback> {
        self.applications += 1;
        (self.application)(view)
    }
    fn render_help(
        &mut self,
        window: &str,
        rows: &[help::HelpRow<'_>],
        close_key: &str,
        requested_scroll: u16,
    ) -> UiResult<RenderFeedback> {
        Ok(RenderFeedback::default())
    }
    fn render_launcher(&mut self, view: &launcher::LauncherView) -> UiResult<RenderFeedback> {
        Ok(RenderFeedback::default())
    }
    fn render_session_picker(
        &mut self,
        scope: &str,
        sessions: &[styra_protocol::SessionSummary],
        selected: usize,
        order: picker::SessionOrder,
        preview: picker::Preview<'_>,
        filter: Option<&str>,
        searching: bool,
        show_completed: bool,
    ) -> UiResult<RenderFeedback> {
        Ok(RenderFeedback::default())
    }
    fn render_session_picker_message(
        &mut self,
        scope: &str,
        sessions: &[styra_protocol::SessionSummary],
        selected: usize,
        order: picker::SessionOrder,
        preview: picker::Preview<'_>,
        filter: Option<&str>,
        searching: bool,
        show_completed: bool,
        title: &str,
        message: &str,
    ) -> UiResult<RenderFeedback> {
        Ok(RenderFeedback::default())
    }
    fn render_session_picker_name_prompt(
        &mut self,
        scope: &str,
        sessions: &[styra_protocol::SessionSummary],
        selected: usize,
        order: picker::SessionOrder,
        preview: picker::Preview<'_>,
        filter: Option<&str>,
        searching: bool,
        show_completed: bool,
        value: &str,
    ) -> UiResult<RenderFeedback> {
        Ok(RenderFeedback::default())
    }
    fn render_workspace_picker(
        &mut self,
        workspaces: &[styra_protocol::WorkspaceSummary],
        selected: usize,
        interactions: &[styra_protocol::InteractionSummary],
        preview: picker::SessionsPreview<'_>,
        filter: Option<&str>,
        offer: Option<&std::path::Path>,
        show_deleted: bool,
    ) -> UiResult<RenderFeedback> {
        Ok(RenderFeedback::default())
    }
    fn render_fuzzy_picker(
        &mut self,
        view: &fuzzy_list::FuzzyListView<'_>,
    ) -> UiResult<RenderFeedback> {
        Ok(RenderFeedback::default())
    }
    fn render_worktree_picker(
        &mut self,
        view: &worktrees::WorktreePickerView<'_>,
    ) -> UiResult<RenderFeedback> {
        Ok(RenderFeedback::default())
    }
    fn render_workspace_picker_prompt(
        &mut self,
        workspaces: &[styra_protocol::WorkspaceSummary],
        selected: usize,
        interactions: &[styra_protocol::InteractionSummary],
        preview: picker::SessionsPreview<'_>,
        filter: Option<&str>,
        title: &str,
        value: &str,
    ) -> UiResult<RenderFeedback> {
        Ok(RenderFeedback::default())
    }
    fn render_template_picker(
        &mut self,
        templates: &[styra_protocol::TemplateSummary],
        chosen: &[String],
        cursor: usize,
    ) -> UiResult<RenderFeedback> {
        Ok(RenderFeedback::default())
    }
    fn render_template_picker_loading(&mut self) -> UiResult<RenderFeedback> {
        Ok(RenderFeedback::default())
    }
    fn poll_event(&mut self, timeout: Duration) -> UiResult<Option<crossterm::event::Event>> {
        self.events
            .pop_front()
            .map(Some)
            .ok_or_else(|| UiError::new("test input exhausted"))
    }
    fn set_bracketed_paste(&mut self, _enabled: bool) -> UiResult<()> {
        Ok(())
    }
    fn close(&mut self) -> UiResult<()> {
        Ok(())
    }
}
