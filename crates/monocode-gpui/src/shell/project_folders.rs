//! The Focused sidebar's optional project list: every open project is a row
//! that folds, with its chats listed under it. It takes the place of the
//! project picker and the spaces bar.
use super::*;
use std::time::Duration;

/// How long a folded project is hovered before its chats pop out.
const FLYOUT_OPEN_DELAY: Duration = Duration::from_millis(300);
/// Time to cross the gap between a folded project and its popover.
const FLYOUT_CLOSE_DELAY: Duration = Duration::from_millis(200);

/// How often an open popover checks that the pointer is still on it.
const FLYOUT_WATCH_INTERVAL: Duration = Duration::from_millis(120);
/// Checks in a row with the pointer elsewhere before the popover closes.
const FLYOUT_WATCH_MISSES: u32 = 3;

/// The strip of project icons a collapsed sidebar leaves behind.
pub(super) const PROJECT_RAIL_WIDTH: f32 = 44.;
/// Chats listed under an open project before its "All chats" row.
const FOLDER_LIMIT: usize = 5;
/// Chats listed in a popover before its "All chats" row.
const FLYOUT_LIMIT: usize = 10;

/// The popover listing a folded project's chats.
#[derive(Default)]
pub(super) struct ProjectFlyout {
    open: Option<String>,
    rows: HashMap<String, AnchorSlot>,
    surface: AnchorSlot,
    open_task: Option<gpui::Task<()>>,
    close_task: Option<gpui::Task<()>>,
    watch_task: Option<gpui::Task<()>>,
    /// Chats opened since their project was last folded. A folded project
    /// lists these, while their tabs stay open, under its row.
    peeked: HashSet<i64>,
    /// Rows listed only because their chat is open, which get a minus button.
    pub(super) minus_rows: HashSet<i64>,
}

impl ProjectFlyout {
    fn row_contains(&self, path: &str, position: Point<Pixels>) -> bool {
        self.rows
            .get(path)
            .and_then(AnchorSlot::get)
            .is_some_and(|bounds| bounds.contains(&position))
    }

    pub(super) fn track_row(&mut self, path: &str) {
        self.rows.entry(path.to_owned()).or_default();
    }
}

impl Shell {
    pub(super) fn project_folder_row_contains(&self, path: &str, at: Point<Pixels>) -> bool {
        self.project_flyout.row_contains(path, at)
    }

    /// A collapsed sidebar leaves its projects as a strip of icons.
    pub(super) fn project_rail_showing(&self) -> bool {
        self.panels_swapped()
            && self.project_folders_active()
            && !self.workspace.sidebar_visible
            && self.settings.is_none()
    }

    fn project_flyout_allowed(&self, path: &str) -> bool {
        self.project_rail_showing() || !self.project_folder_expanded(path)
    }

    /// The strip itself. It is drawn from the moment the sidebar starts to
    /// close, underneath it, so the sidebar folds away onto it.
    pub(super) fn project_icon_rail(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.panels_swapped()
            || !self.project_folders_active()
            || self.settings.is_some()
            || self.sidebar_fraction >= 1.
        {
            return None;
        }
        let projects = self.workspace.projects.clone();
        for path in &projects {
            self.project_flyout.track_row(path);
        }
        let icons: Vec<_> = projects
            .iter()
            .enumerate()
            .map(|(ix, path)| self.project_rail_icon(ix, path, cx))
            .collect();
        let flyout = self
            .project_rail_showing()
            .then(|| self.project_flyout_popover(cx))
            .flatten();
        Some(
            div()
                .id("project-icon-rail")
                .debug_selector(|| "project-icon-rail".into())
                .role(Role::Group)
                .aria_label("Projects")
                .absolute()
                .top_0()
                .right_0()
                .bottom_0()
                .w(px(PROJECT_RAIL_WIDTH))
                .opacity(1. - self.sidebar_fraction)
                .bg(inset_chrome_wash())
                .pt_1()
                .flex()
                .flex_col()
                .items_center()
                .gap_1()
                .overflow_y_scroll()
                .children(icons)
                .children(flyout)
                .into_any_element(),
        )
    }

    fn project_rail_icon(&self, ix: usize, path: &str, cx: &mut Context<Self>) -> AnyElement {
        let current = path == self.workspace.project;
        let hover_path = path.to_owned();
        let menu_path = path.to_owned();
        let open_path = path.to_owned();
        let selector = format!("project-rail-{path}");
        div()
            .id(("project-rail-icon", ix))
            .debug_selector(move || selector.clone())
            .role(Role::Button)
            .aria_label(self.project_label(path))
            .aria_description(path.to_owned())
            .relative()
            .flex_shrink_0()
            .size(px(32.))
            .rounded_md()
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .when(current, |d| d.bg(content(0.1)))
            .hover(|s| s.bg(content(0.08)))
            .when(
                self.sidebar_drop == Some(session_groups::ListDrop::Project(path.to_owned())),
                |d| d.bg(content(0.14)),
            )
            .children(
                self.project_flyout
                    .rows
                    .get(path)
                    .map(|slot| slot.probe().into_any_element()),
            )
            .child(self.project_mark(path, 18.))
            .on_hover(cx.listener(move |this, hovered: &bool, window, cx| {
                this.hover_project_folder(&hover_path, *hovered, window, cx);
            }))
            .on_click(cx.listener(move |this, _, window, cx| {
                this.close_project_flyout(cx);
                if open_path != this.workspace.project {
                    this.select_project(open_path.clone(), window, cx);
                }
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    this.close_project_flyout(cx);
                    let target = MenuTarget::Project {
                        path: menu_path.clone(),
                        rail: false,
                    };
                    this.open_menu(target, Anchor::Point(event.position), window, cx);
                }),
            )
            .into_any_element()
    }

    pub(super) fn project_folders_active(&self) -> bool {
        self.workspace.sidebar_project_folders && !self.workspace.classic_layout
    }

    /// Whether the sidebar shows every project. Otherwise it shows the single
    /// project chosen from an "All chats" row, or Project folders is off.
    pub(super) fn project_list_showing(&self) -> bool {
        self.project_folders_active()
            && self.workspace.sidebar_focused_project.as_ref() != Some(&self.workspace.project)
    }

    /// Opening a chat from elsewhere, such as a notification, can change the
    /// project. The sidebar then goes back to the list and stays there.
    pub(super) fn end_stale_project_focus(&mut self) {
        if self
            .workspace
            .sidebar_focused_project
            .as_ref()
            .is_some_and(|path| *path != self.workspace.project)
        {
            self.workspace.sidebar_focused_project = None;
        }
    }

    fn focus_project(&mut self, path: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.close_preview(cx);
        if path != self.workspace.project {
            self.select_project(path.to_owned(), window, cx);
        }
        self.workspace.sidebar_focused_project = Some(path.to_owned());
        self.persist(cx);
        self.invalidate_chrome(cx);
        cx.notify();
    }

    fn leave_project_focus(&mut self, cx: &mut Context<Self>) {
        self.close_preview(cx);
        self.workspace.sidebar_focused_project = None;
        self.persist(cx);
        self.invalidate_chrome(cx);
        cx.notify();
    }

    pub(super) fn project_folder_expanded(&self, path: &str) -> bool {
        !self.query.is_empty() || !self.workspace.collapsed.contains(path)
    }

    /// Every project in saved order with its listed sessions, pins first and
    /// then newest first.
    pub(super) fn project_folder_sessions(&self, cx: &App) -> Vec<(String, Vec<i64>)> {
        let projects: HashSet<&str> = self.workspace.projects.iter().map(String::as_str).collect();
        let mut listed: HashMap<String, Vec<i64>> = HashMap::new();
        for id in self.listed_session_ids(|cwd| projects.contains(cwd), cx) {
            listed
                .entry(self.session_project(id, cx).to_owned())
                .or_default()
                .push(id);
        }
        self.workspace
            .projects
            .iter()
            .map(|path| {
                let mut ids = listed.remove(path).unwrap_or_default();
                ids.sort_by_key(|id| !self.session_metadata(*id, cx).summary.pinned);
                (path.clone(), ids)
            })
            .collect()
    }

    /// The sessions on show, top to bottom, across every folder.
    pub(super) fn project_folder_visible_ids(&self, cx: &App) -> Vec<i64> {
        self.project_folder_sessions(cx)
            .into_iter()
            .flat_map(|(path, ids)| self.project_folder_members(&path, &ids, cx))
            .map(|(id, _)| id)
            .collect()
    }

    /// The chats listed under a project's row: its pins and newest chats while
    /// open, and any chat with a tab, so a chat in use stays in reach. Search
    /// lists every match. The flag marks a chat listed only for its tab, which
    /// gets a button to close it back into the folder.
    pub(super) fn project_folder_members(
        &self,
        path: &str,
        sessions: &[i64],
        cx: &App,
    ) -> Vec<(i64, bool)> {
        if !self.query.is_empty() {
            return sessions.iter().map(|id| (*id, false)).collect();
        }
        let limit = if self.project_folder_expanded(path) {
            let pinned = sessions
                .iter()
                .filter(|id| self.session_metadata(**id, cx).summary.pinned)
                .count();
            pinned.max(FOLDER_LIMIT)
        } else {
            0
        };
        let open = self.workspace.open_sessions();
        let peeked = |id: i64| {
            open.contains(&id)
                && (self.project_flyout.peeked.contains(&id) || self.workspace.active == Some(id))
        };
        sessions
            .iter()
            .enumerate()
            .filter(|(ix, id)| *ix < limit || peeked(**id))
            .map(|(ix, id)| (*id, ix >= limit))
            .collect()
    }

    /// The minus button that closes a chat listed only because it is open, so
    /// it goes back into its folder. It takes the place of the row's time
    /// while the row is hovered or in use.
    pub(super) fn peeked_close_button(&self, id: i64, cx: &Context<Self>) -> Option<AnyElement> {
        let active = self.workspace.active == Some(id) && !self.mono_active(cx);
        if !self.project_flyout.minus_rows.contains(&id)
            || !(active || self.session_hover == Some(id))
            || !self.tab_can_close(id, cx)
        {
            return None;
        }
        Some(
            div()
                .id(SharedString::from(format!("peeked-session-close-{id}")))
                .debug_selector(move || format!("peeked-session-close-{id}"))
                .role(Role::Button)
                .aria_label("Close chat")
                .cursor_pointer()
                .size(px(20.))
                .mr(px(-3.))
                .flex_shrink_0()
                .rounded_md()
                .flex()
                .items_center()
                .justify_center()
                .bg(content(0.1))
                .text_color(content(0.7))
                .hover(|s| s.bg(content(0.16)).text_color(content(1.)))
                .child(icon("minus", 12.))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(move |this, _, window, cx| {
                    cx.stop_propagation();
                    this.close(id, window, cx);
                    if this.workspace.tab_of(id).is_none() {
                        this.project_flyout.peeked.remove(&id);
                    }
                }))
                .into_any(),
        )
    }

    fn hover_project_folder(
        &mut self,
        path: &str,
        hovered: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        if !hovered {
            self.project_flyout.open_task = None;
            // Whichever row the pointer left, an open popover must not stay.
            if self.project_flyout.open.is_some() {
                self.schedule_project_flyout_close(window, cx);
            }
            return;
        }
        // Reaching another row leaves the old popover's close timer running.
        if self.project_flyout.open.as_deref() == Some(path) {
            self.project_flyout.close_task = None;
            return;
        }
        if !self.project_flyout_allowed(path)
            || self.menu.is_some()
            || self.pane_drag.is_some()
            || self.tab_drag.is_some()
        {
            return;
        }
        let path = path.to_owned();
        self.project_flyout.open_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(FLYOUT_OPEN_DELAY).await;
            let _ = this.update_in(cx, |this, window, cx| {
                // The pointer may have left, or a menu opened, during the wait.
                let over_row = this
                    .project_flyout
                    .rows
                    .get(&path)
                    .and_then(AnchorSlot::get)
                    .is_some_and(|bounds| bounds.contains(&window.mouse_position()));
                if over_row && this.menu.is_none() && this.project_flyout_allowed(&path) {
                    this.close_preview(cx);
                    this.project_flyout.open = Some(path.clone());
                    this.project_flyout.close_task = None;
                    this.project_flyout.surface = AnchorSlot::default();
                    this.watch_project_flyout(cx);
                    this.sidebar_frame.update(cx, |_, cx| cx.notify());
                    // The icon strip is drawn by the window, not the sidebar.
                    cx.notify();
                }
            });
        }));
    }

    /// Hover events can be missed, such as when a row is redrawn under the
    /// pointer. This looks at where the pointer really is, so the popover
    /// always goes away.
    fn watch_project_flyout(&mut self, cx: &mut Context<Self>) {
        self.project_flyout.watch_task = Some(cx.spawn(async move |this, cx| {
            let mut away = 0;
            loop {
                cx.background_executor().timer(FLYOUT_WATCH_INTERVAL).await;
                let keep = this.update_in(cx, |this, window, cx| {
                    let Some(path) = this.project_flyout.open.clone() else {
                        return false;
                    };
                    let at = window.mouse_position();
                    let over_popover = this
                        .project_flyout
                        .surface
                        .get()
                        .is_some_and(|bounds| bounds.contains(&at));
                    if over_popover || this.project_flyout.row_contains(&path, at) {
                        away = 0;
                        return true;
                    }
                    away += 1;
                    if away < FLYOUT_WATCH_MISSES {
                        return true;
                    }
                    this.close_project_flyout(cx);
                    false
                });
                if !matches!(keep, Ok(true)) {
                    return;
                }
            }
        }));
    }

    fn schedule_project_flyout_close(&mut self, window: &Window, cx: &mut Context<Self>) {
        // The popover can already sit under the pointer when the row reports
        // that the pointer left.
        if self
            .project_flyout
            .surface
            .get()
            .is_some_and(|bounds| bounds.contains(&window.mouse_position()))
        {
            self.project_flyout.close_task = None;
            return;
        }
        self.project_flyout.close_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(FLYOUT_CLOSE_DELAY).await;
            let _ = this.update(cx, |this, cx| this.close_project_flyout(cx));
        }));
    }

    pub(super) fn close_project_flyout(&mut self, cx: &mut Context<Self>) {
        self.project_flyout.open_task = None;
        self.project_flyout.close_task = None;
        self.project_flyout.watch_task = None;
        if self.project_flyout.open.take().is_some() {
            self.sidebar_frame.update(cx, |_, cx| cx.notify());
            cx.notify();
        }
    }

    pub(super) fn project_flyout_popover(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let path = self.project_flyout.open.clone()?;
        let anchor = self.project_flyout.rows.get(&path)?.clone();
        let sessions = self
            .project_folder_sessions(cx)
            .into_iter()
            .find(|(project, _)| *project == path)
            .map_or_else(Vec::new, |(_, ids)| ids);
        let row = |id: SharedString| {
            div()
                .id(id.clone())
                .debug_selector(move || id.to_string())
                .role(Role::MenuItem)
                .cursor_pointer()
                .h(px(32.))
                .px_2()
                .rounded_md()
                .flex()
                .items_center()
                .gap_2()
                .text_size(px(13.))
                .hover(|s| s.bg(content(0.08)))
        };
        let new_path = path.clone();
        let new_chat = row("project-flyout-new".into())
            .aria_label("New session")
            .text_color(content(0.55))
            .child(icon("plus", 14.))
            .child("New chat")
            .on_click(cx.listener(move |this, _, window, cx| {
                this.close_project_flyout(cx);
                this.new_chat_in_project(&new_path, window, cx);
            }));
        let total = sessions.len();
        let all_path = path.clone();
        let all_chats = (total > FLYOUT_LIMIT).then(|| {
            row("project-flyout-all".into())
                .aria_label(format!("All {total} chats"))
                .text_color(content(0.55))
                .child(div().flex_1().child(format!("All {total} chats")))
                .child(icon("chevron-right", 12.))
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.focus_project(&all_path, window, cx);
                }))
        });
        let chats: Vec<_> = sessions
            .iter()
            .take(FLYOUT_LIMIT)
            .map(|id| {
                let id = *id;
                let title = self.session_metadata(id, cx).display_title();
                // Only chats opened since launch have a live mark.
                let mark = match self.session_marks.get(&id) {
                    Some(mark) => {
                        mark.update(cx, |mark, cx| mark.set_size(14., cx));
                        mark.clone().into_any_element()
                    }
                    None => {
                        let provider = &self.session_metadata(id, cx).summary.provider;
                        provider_icon(provider, 14.).into_any_element()
                    }
                };
                let mark = div()
                    .debug_selector(move || format!("project-flyout-mark-{id}"))
                    .size(px(14.))
                    .flex_shrink_0()
                    .child(mark);
                row(format!("project-flyout-session-{id}").into())
                    .aria_label(title.clone())
                    .child(mark)
                    .child(div().flex_1().min_w_0().truncate().child(title))
                    .on_click(
                        cx.listener(move |this, event: &gpui::ClickEvent, window, cx| {
                            this.close_project_flyout(cx);
                            this.select_sidebar_session(id, event.modifiers(), window, cx);
                        }),
                    )
            })
            .collect();
        Some(
            popover("project-flyout", Anchor::Slot(anchor))
                .side(if self.panels_swapped() {
                    Side::Left
                } else {
                    Side::Right
                })
                .align(Align::Start)
                .gap(8.)
                .width(260.)
                .max_height(440.)
                .padding(4.)
                .allow_outside_press()
                .track(self.project_flyout.surface.clone())
                .child(
                    div()
                        .id("project-flyout-content")
                        .debug_selector(|| "project-flyout".into())
                        .role(Role::Menu)
                        .aria_label(format!("{} chats", self.project_label(&path)))
                        .max_h(px(432.))
                        .overflow_y_scroll()
                        .flex()
                        .flex_col()
                        .gap_px()
                        .on_hover(cx.listener(|this, hovered: &bool, window, cx| {
                            if *hovered {
                                this.project_flyout.close_task = None;
                            } else {
                                this.schedule_project_flyout_close(window, cx);
                            }
                        }))
                        .when(self.project_rail_showing(), |d| {
                            // The strip shows only an icon, so name the project.
                            d.child(
                                div()
                                    .debug_selector(|| "project-flyout-name".into())
                                    .h(px(28.))
                                    .px_2()
                                    .flex()
                                    .items_center()
                                    .text_size(px(12.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(content(0.6))
                                    .child(self.project_label(&path)),
                            )
                        })
                        .child(new_chat)
                        .children(chats)
                        .children(all_chats),
                )
                .into_any_element(),
        )
    }

    pub(super) fn note_project_chat_opened(&mut self, id: i64, cx: &mut Context<Self>) {
        if self.project_flyout.peeked.insert(id) {
            self.notify_sidebar(cx);
        }
    }

    fn toggle_project_folder(&mut self, path: &str, cx: &mut Context<Self>) {
        if !self.query.is_empty() {
            return;
        }
        if !self.workspace.collapsed.remove(path) {
            self.workspace.collapsed.insert(path.to_owned());
            // Folding tucks the project's chats away, except the one in use.
            let active = self.workspace.active;
            let peeked = std::mem::take(&mut self.project_flyout.peeked);
            self.project_flyout.peeked = peeked
                .into_iter()
                .filter(|id| {
                    Some(*id) == active
                        || !self.session_summaries.contains_key(id)
                        || self.session_project(*id, cx) != path
                })
                .collect();
        }
        self.close_preview(cx);
        self.persist(cx);
        self.invalidate_chrome(cx);
        cx.notify();
    }

    /// Starts a chat in `path`. Switching to a project with nothing open
    /// already gives it a blank chat, so that one is used instead of a second.
    fn new_chat_in_project(&mut self, path: &str, window: &mut Window, cx: &mut Context<Self>) {
        if path != self.workspace.project {
            self.select_project(path.to_owned(), window, cx);
            if !self.pending_project_blanks.is_empty() || self.active_session_is_blank(cx) {
                return;
            }
        }
        self.new_chat(cx);
    }

    pub(super) fn project_folders_heading(&mut self, cx: &mut Context<Self>) -> AnyElement {
        section_label("Projects")
            .child(
                icon_button("open-project-menu", "Open project", "plus")
                    .debug_selector(|| "project-folders-open".into())
                    .relative()
                    .aria_expanded(self.menu_open(&MenuTarget::Header))
                    .when(self.menu_open(&MenuTarget::Header), |d| d.bg(content(0.1)))
                    .child(self.project_anchor.probe())
                    .on_click(cx.listener(|this, _, window, cx| {
                        if this.menu_open(&MenuTarget::Header) {
                            this.close_menu(cx);
                        } else {
                            let anchor = Anchor::Slot(this.project_anchor.clone());
                            this.open_menu(MenuTarget::Header, anchor, window, cx);
                        }
                    })),
            )
            .child(self.sidebar_filter_button(cx))
            .into_any()
    }

    pub(super) fn project_folder_all_row(
        &self,
        path: &str,
        total: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let target = path.to_owned();
        let selector = format!("project-folder-all-{path}");
        div()
            .id(SharedString::from(selector.clone()))
            .debug_selector(move || selector.clone())
            .role(Role::Button)
            .aria_label(format!("All {total} chats"))
            .cursor_pointer()
            .ml(px(30.))
            .h(px(28.))
            .px_2()
            .rounded_md()
            .flex()
            .items_center()
            .gap_1()
            .text_size(px(12.))
            .text_color(content(0.5))
            .hover(|s| s.bg(content(0.08)).text_color(content(1.)))
            .child(format!("All {total} chats"))
            .child(icon("chevron-right", 12.))
            .on_click(cx.listener(move |this, _, window, cx| {
                this.focus_project(&target, window, cx);
            }))
            .into_any()
    }

    pub(super) fn project_list_back_button(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .id("project-list-back")
            .debug_selector(|| "project-list-back".into())
            .role(Role::Button)
            .aria_label("Back to projects")
            .cursor_pointer()
            .h(px(28.))
            .px_1p5()
            .ml(px(-4.))
            .rounded_md()
            .flex()
            .items_center()
            .gap_1()
            .text_size(px(13.))
            .font_weight(FontWeight::SEMIBOLD)
            .hover(|s| s.bg(content(0.08)))
            .child(icon("chevron-left", 14.))
            .child("Projects")
            .on_click(cx.listener(|this, _, _, cx| this.leave_project_focus(cx)))
            .into_any()
    }

    /// Names the project at the top of its full view, with the same right-click
    /// menu as its row in the list.
    pub(super) fn focused_project_row(&self, cx: &mut Context<Self>) -> AnyElement {
        let path = self.workspace.project.clone();
        let menu_path = path.clone();
        div()
            .id("focused-project")
            .debug_selector(|| "focused-project".into())
            .h(px(32.))
            .px_2()
            .flex()
            .items_center()
            .gap_2()
            .child(
                div()
                    .size(px(16.))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(self.project_mark(&path, 14.)),
            )
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .text_size(px(13.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(self.project_label(&path)),
            )
            .when_some(self.worktree_focus.get(&path), |d, focus| {
                d.child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_size(px(12.))
                        .text_color(content(0.45))
                        .child(focus.branch.clone().unwrap_or_else(|| "detached".into())),
                )
            })
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    let target = MenuTarget::Project {
                        path: menu_path.clone(),
                        rail: false,
                    };
                    this.open_menu(target, Anchor::Point(event.position), window, cx);
                }),
            )
            .into_any()
    }

    pub(super) fn project_folder_row(
        &self,
        path: &str,
        sessions: &[i64],
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let expanded = self.project_folder_expanded(path);
        let current = path == self.workspace.project;
        let label = self.project_label(path);
        let approval = sessions
            .iter()
            .any(|id| self.session_metadata(*id, cx).needs_approval());
        let done = sessions.iter().any(|id| self.unseen_finished.contains(id));
        let group = SharedString::from(format!("project-folder-group-{path}"));
        let toggle_path = path.to_owned();
        let menu_path = path.to_owned();
        let new_path = path.to_owned();
        let hover_path = path.to_owned();
        let anchor = self.project_flyout.rows.get(path).cloned();
        if let Some(anchor) = &anchor {
            self.sidebar_list.track_anchor(anchor.clone());
        }
        div()
            .id(SharedString::from(format!("project-folder-{path}")))
            .debug_selector({
                let path = path.to_owned();
                move || format!("project-folder-{path}")
            })
            .group(group.clone())
            .role(Role::Button)
            .aria_label(label.clone())
            .aria_description(path.to_owned())
            .aria_expanded(expanded)
            .cursor_pointer()
            .h(px(32.))
            .px_2()
            .rounded_md()
            .relative()
            .flex()
            .items_center()
            .gap_2()
            .children(anchor.map(|slot| slot.probe().into_any_element()))
            .text_color(content(if current { 1. } else { 0.6 }))
            .hover(|s| s.bg(content(0.08)).text_color(content(1.)))
            .when(
                self.sidebar_drop == Some(session_groups::ListDrop::Project(path.to_owned())),
                |d| d.bg(content(0.14)).text_color(content(1.)),
            )
            .child(
                div()
                    .size(px(16.))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(self.project_mark(path, 14.)),
            )
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .text_size(px(13.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(label),
            )
            .when_some(self.worktree_focus.get(path), |d, focus| {
                d.child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_size(px(12.))
                        .text_color(content(0.45))
                        .child(focus.branch.clone().unwrap_or_else(|| "detached".into())),
                )
            })
            .child(
                icon(
                    if expanded {
                        "chevron-down"
                    } else {
                        "chevron-right"
                    },
                    12.,
                )
                .flex_shrink_0()
                .opacity(0.6),
            )
            .child(div().flex_1())
            .children(self.agent_changes_badge(path, cx))
            .when(!expanded && approval, |d| {
                d.child(icon("circle-alert", 12.).text_color(rgb(0xfbbf24)))
            })
            .when(!expanded && !approval && done, |d| {
                d.child(icon("check-circle-2", 12.).text_color(rgb(0x2dd4bf)))
            })
            .child(
                div()
                    .relative()
                    .flex_shrink_0()
                    .size(px(20.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(content(0.4))
                            .group_hover(group.clone(), |s| s.invisible())
                            .child(sessions.len().to_string()),
                    )
                    .child(
                        div()
                            .id(SharedString::from(format!("project-folder-new-{path}")))
                            .debug_selector({
                                let path = path.to_owned();
                                move || format!("project-folder-new-{path}")
                            })
                            .role(Role::Button)
                            .aria_label("New session")
                            .absolute()
                            .inset_0()
                            .rounded_md()
                            .flex()
                            .items_center()
                            .justify_center()
                            .invisible()
                            .group_hover(group, |s| s.visible())
                            .hover(|s| s.bg(content(0.1)))
                            .child(icon("plus", 14.))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                cx.stop_propagation();
                                this.new_chat_in_project(&new_path, window, cx);
                            })),
                    ),
            )
            .on_hover(cx.listener(move |this, hovered: &bool, window, cx| {
                this.hover_project_folder(&hover_path, *hovered, window, cx);
            }))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.toggle_project_folder(&toggle_path, cx);
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    this.close_project_flyout(cx);
                    let target = MenuTarget::Project {
                        path: menu_path.clone(),
                        rail: false,
                    };
                    this.open_menu(target, Anchor::Point(event.position), window, cx);
                }),
            )
            .into_any()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;

    const OTHER: &str = "/work/other";

    fn chat(shell: &mut Shell, cwd: &str, title: &str, updated_at: u64) -> i64 {
        let mut summary = locked(&shell.store).unwrap().create("codex", cwd).unwrap();
        summary.has_messages = true;
        summary.title = title.into();
        summary.updated_at = updated_at;
        let id = summary.id;
        shell.session_summaries.insert(id, summary);
        shell.order.insert(0, id);
        id
    }

    /// Two projects with chats. The window's own project is listed first.
    fn setup(cx: &mut TestAppContext) -> (gpui::WindowHandle<Shell>, String, i64, i64, i64) {
        let (window, _, _) = super::super::tests::window(cx);
        let ids = window
            .update(cx, |shell, _, cx| {
                shell.workspace.sidebar_project_folders = true;
                shell.workspace.add_project(OTHER);
                let home = shell.workspace.project.clone();
                let old = chat(shell, &home, "Older here", 1_000);
                let new = chat(shell, &home, "Newer here", 2_000);
                let other = chat(shell, OTHER, "Over there", 3_000);
                shell.layout_changed(cx);
                shell.notify_sidebar(cx);
                (home, old, new, other)
            })
            .unwrap();
        cx.run_until_parked();
        (window, ids.0, ids.1, ids.2, ids.3)
    }

    fn selector(prefix: &str, value: impl std::fmt::Display) -> &'static str {
        Box::leak(format!("{prefix}{value}").into_boxed_str())
    }

    #[gpui::test]
    fn every_project_lists_its_chats_and_the_picker_goes_away(cx: &mut TestAppContext) {
        let (window, home, old, new, other) = setup(cx);
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();
        for id in [old, new, other] {
            assert!(
                visual
                    .debug_bounds(selector("sidebar-session-", id))
                    .is_some()
            );
        }
        assert!(
            visual
                .debug_bounds(selector("project-folder-", OTHER))
                .is_some()
        );
        assert!(visual.debug_bounds("sidebar-project-button").is_none());
        assert!(visual.debug_bounds("project-space-0").is_none());
        window
            .update(visual, |shell, _, cx| {
                let folders = shell.project_folder_sessions(cx);
                assert!(folders.contains(&(home.clone(), vec![new, old])));
                assert!(folders.contains(&(OTHER.into(), vec![other])));
                // Saved order, so "Move" in the project menu moves the row.
                let shown: Vec<_> = folders.into_iter().map(|(path, _)| path).collect();
                assert_eq!(shown, shell.workspace.projects);
            })
            .unwrap();
    }

    #[gpui::test]
    fn pins_lead_their_project(cx: &mut TestAppContext) {
        let (window, _, old, new, _) = setup(cx);
        window
            .update(cx, |shell, _, cx| {
                shell.session_summaries.get_mut(&old).unwrap().pinned = true;
                let home = shell.workspace.project.clone();
                let folders = shell.project_folder_sessions(cx);
                assert!(folders.contains(&(home, vec![old, new])));
            })
            .unwrap();
    }

    #[gpui::test]
    fn folders_fold_without_switching_project_and_stay_folded(cx: &mut TestAppContext) {
        let (window, home, _, _, other) = setup(cx);
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();
        let row = visual
            .debug_bounds(selector("project-folder-", OTHER))
            .unwrap();
        visual.simulate_click(row.center(), Default::default());
        visual.run_until_parked();
        assert!(
            visual
                .debug_bounds(selector("sidebar-session-", other))
                .is_none()
        );
        window
            .update(visual, |shell, _, cx| {
                assert_eq!(shell.workspace.project, home);
                assert!(!shell.sidebar_session_ids(cx).contains(&other));
                let saved = shell.latest_state.lock().unwrap().clone();
                let restored = Workspace::restore(Some(&saved), &[], &home);
                assert!(restored.collapsed.contains(OTHER));
                assert!(restored.sidebar_project_folders);
            })
            .unwrap();
        let row = visual
            .debug_bounds(selector("project-folder-", OTHER))
            .unwrap();
        visual.simulate_click(row.center(), Default::default());
        visual.run_until_parked();
        assert!(
            visual
                .debug_bounds(selector("sidebar-session-", other))
                .is_some()
        );
    }

    #[gpui::test]
    fn clicking_a_chat_in_another_project_switches_to_it(cx: &mut TestAppContext) {
        let (window, _, _, _, other) = setup(cx);
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();
        let row = visual
            .debug_bounds(selector("sidebar-session-", other))
            .unwrap();
        visual.simulate_click(row.center(), Default::default());
        visual.run_until_parked();
        window
            .update(visual, |shell, _, _| {
                assert_eq!(shell.workspace.project, OTHER);
                assert_eq!(shell.workspace.active, Some(other));
            })
            .unwrap();
    }

    #[gpui::test]
    fn the_plus_starts_a_chat_in_that_project_without_folding_it(cx: &mut TestAppContext) {
        let (window, _, _, _, other) = setup(cx);
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();
        let row = visual
            .debug_bounds(selector("project-folder-", OTHER))
            .unwrap();
        visual.simulate_mouse_move(row.center(), None, Default::default());
        visual.run_until_parked();
        let plus = visual
            .debug_bounds(selector("project-folder-new-", OTHER))
            .unwrap();
        visual.simulate_click(plus.center(), Default::default());
        visual.run_until_parked();
        window
            .update(visual, |shell, _, cx| {
                assert_eq!(shell.workspace.project, OTHER);
                assert!(!shell.workspace.collapsed.contains(OTHER));
                let active = shell.workspace.active.unwrap();
                assert_ne!(active, other);
                let session = shell.session_metadata(active, cx).summary;
                assert_eq!(session.cwd, OTHER);
                assert!(!session.has_messages);
                let blank = shell
                    .order
                    .iter()
                    .filter(|id| {
                        let session = shell.session_metadata(**id, cx).summary;
                        session.cwd == OTHER && !session.has_messages
                    })
                    .count();
                assert_eq!(blank, 1, "the switch must not leave a second blank chat");
            })
            .unwrap();
    }

    #[gpui::test]
    fn search_opens_folded_projects_and_hides_the_rest(cx: &mut TestAppContext) {
        let (window, home, old, _, other) = setup(cx);
        window
            .update(cx, |shell, _, cx| {
                shell.workspace.collapsed.insert(OTHER.into());
                shell.query = "over".into();
                assert_eq!(shell.sidebar_session_ids(cx), [other]);
                shell.notify_sidebar(cx);
            })
            .unwrap();
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();
        assert!(
            visual
                .debug_bounds(selector("sidebar-session-", other))
                .is_some()
        );
        assert!(
            visual
                .debug_bounds(selector("sidebar-session-", old))
                .is_none()
        );
        assert!(
            visual
                .debug_bounds(selector("project-folder-", &home))
                .is_none()
        );
    }

    /// Folds OTHER, then rests the pointer on its row past the hover delay.
    fn hover_folded(window: gpui::WindowHandle<Shell>, visual: &mut gpui::VisualTestContext) {
        hover_folded_project(window, visual, OTHER);
    }

    fn hover_folded_project(
        window: gpui::WindowHandle<Shell>,
        visual: &mut gpui::VisualTestContext,
        path: &str,
    ) {
        window
            .update(visual, |shell, _, cx| {
                shell.workspace.collapsed.insert(path.into());
                shell.notify_sidebar(cx);
            })
            .unwrap();
        visual.run_until_parked();
        let row = visual
            .debug_bounds(selector("project-folder-", path))
            .unwrap();
        visual.simulate_mouse_move(row.center(), None, Default::default());
        visual.run_until_parked();
        visual.executor().advance_clock(FLYOUT_OPEN_DELAY);
        visual.run_until_parked();
    }

    #[gpui::test]
    fn hovering_a_folded_project_pops_out_its_chats_after_a_delay(cx: &mut TestAppContext) {
        let (window, _, _, _, other) = setup(cx);
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();
        // An open folder already shows its chats, so it has no popover.
        let row = visual
            .debug_bounds(selector("project-folder-", OTHER))
            .unwrap();
        visual.simulate_mouse_move(row.center(), None, Default::default());
        visual.executor().advance_clock(FLYOUT_OPEN_DELAY);
        visual.run_until_parked();
        assert!(visual.debug_bounds("project-flyout").is_none());
        // Moving away before the delay ends cancels it.
        window
            .update(visual, |shell, _, cx| {
                shell.workspace.collapsed.insert(OTHER.into());
                shell.notify_sidebar(cx);
            })
            .unwrap();
        visual.run_until_parked();
        visual.simulate_mouse_move(gpui::point(px(900.), px(500.)), None, Default::default());
        visual.run_until_parked();
        let row = visual
            .debug_bounds(selector("project-folder-", OTHER))
            .unwrap();
        visual.simulate_mouse_move(row.center(), None, Default::default());
        visual.simulate_mouse_move(gpui::point(px(900.), px(500.)), None, Default::default());
        visual.executor().advance_clock(FLYOUT_OPEN_DELAY);
        visual.run_until_parked();
        assert!(visual.debug_bounds("project-flyout").is_none());

        hover_folded(window, visual);
        assert!(visual.debug_bounds("project-flyout").is_some());
        assert!(
            visual
                .debug_bounds(selector("project-flyout-session-", other))
                .is_some()
        );
        // The pointer can cross to the popover without closing it.
        let popover = visual.debug_bounds("project-flyout").unwrap();
        visual.simulate_mouse_move(popover.center(), None, Default::default());
        visual.executor().advance_clock(FLYOUT_CLOSE_DELAY);
        visual.run_until_parked();
        assert!(visual.debug_bounds("project-flyout").is_some());
        // Leaving both closes it.
        visual.simulate_mouse_move(gpui::point(px(900.), px(700.)), None, Default::default());
        visual.run_until_parked();
        visual.executor().advance_clock(FLYOUT_CLOSE_DELAY);
        visual.run_until_parked();
        assert!(visual.debug_bounds("project-flyout").is_none());
    }

    #[gpui::test]
    fn picking_from_the_popover_keeps_the_project_folded_and_shows_only_that_chat(
        cx: &mut TestAppContext,
    ) {
        let (window, _, _, _, other) = setup(cx);
        let second = window
            .update(cx, |shell, _, cx| {
                let id = chat(shell, OTHER, "Also there", 500);
                shell.notify_sidebar(cx);
                id
            })
            .unwrap();
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();
        hover_folded(window, visual);
        assert!(
            visual
                .debug_bounds(selector("sidebar-session-", second))
                .is_none()
        );
        let pick = visual
            .debug_bounds(selector("project-flyout-session-", second))
            .unwrap();
        visual.simulate_click(pick.center(), Default::default());
        visual.run_until_parked();
        assert!(visual.debug_bounds("project-flyout").is_none());
        assert!(
            visual
                .debug_bounds(selector("sidebar-session-", second))
                .is_some()
        );
        assert!(
            visual
                .debug_bounds(selector("sidebar-session-", other))
                .is_none()
        );
        window
            .update(visual, |shell, _, cx| {
                assert_eq!(shell.workspace.project, OTHER);
                assert_eq!(shell.workspace.active, Some(second));
                assert!(shell.workspace.collapsed.contains(OTHER));
                assert!(shell.sidebar_session_ids(cx).contains(&second));
            })
            .unwrap();
    }

    #[gpui::test]
    fn every_chat_in_the_popover_has_an_icon_even_if_never_opened(cx: &mut TestAppContext) {
        let (window, _, _, _, other) = setup(cx);
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();
        window
            .update(visual, |shell, _, _| {
                assert!(!shell.session_marks.contains_key(&other));
            })
            .unwrap();
        hover_folded(window, visual);
        let mark = visual
            .debug_bounds(selector("project-flyout-mark-", other))
            .unwrap();
        assert_eq!(mark.size.width, px(14.));
    }

    #[gpui::test]
    fn folded_projects_each_keep_their_open_chats(cx: &mut TestAppContext) {
        let (window, home, old, new, other) = setup(cx);
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();
        hover_folded(window, visual);
        let pick = visual
            .debug_bounds(selector("project-flyout-session-", other))
            .unwrap();
        visual.simulate_click(pick.center(), Default::default());
        visual.run_until_parked();
        // Picking in a second folded project keeps the first one's chat.
        hover_folded_project(window, visual, &home);
        let pick = visual
            .debug_bounds(selector("project-flyout-session-", old))
            .unwrap();
        visual.simulate_click(pick.center(), Default::default());
        visual.run_until_parked();
        for id in [other, old] {
            assert!(
                visual
                    .debug_bounds(selector("sidebar-session-", id))
                    .is_some()
            );
        }
        assert!(
            visual
                .debug_bounds(selector("sidebar-session-", new))
                .is_none()
        );
        // Closing a chat's tab takes it out of its folded project.
        window
            .update(visual, |shell, window, cx| shell.close(other, window, cx))
            .unwrap();
        visual.run_until_parked();
        assert!(
            visual
                .debug_bounds(selector("sidebar-session-", other))
                .is_none()
        );
    }

    /// Adds chats to OTHER, newest last, and returns their ids.
    fn many_chats(
        window: gpui::WindowHandle<Shell>,
        cx: &mut TestAppContext,
        count: u64,
    ) -> Vec<i64> {
        window
            .update(cx, |shell, _, cx| {
                let ids = (0..count)
                    .map(|n| chat(shell, OTHER, &format!("Chat {n}"), 10_000 + n))
                    .collect();
                shell.notify_sidebar(cx);
                ids
            })
            .unwrap()
    }

    #[gpui::test]
    fn a_long_project_lists_pins_and_newest_chats_then_an_all_row(cx: &mut TestAppContext) {
        let (window, _, _, _, oldest) = setup(cx);
        let ids = many_chats(window, cx, 7);
        window
            .update(cx, |shell, _, cx| {
                shell.session_summaries.get_mut(&ids[0]).unwrap().pinned = true;
                shell.notify_sidebar(cx);
            })
            .unwrap();
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();
        // The pin, then the 4 newest. Older chats wait behind the "All" row.
        for id in [ids[0], ids[6], ids[5], ids[4], ids[3]] {
            assert!(
                visual
                    .debug_bounds(selector("sidebar-session-", id))
                    .is_some()
            );
        }
        for id in [ids[2], ids[1], oldest] {
            assert!(
                visual
                    .debug_bounds(selector("sidebar-session-", id))
                    .is_none()
            );
        }
        assert!(
            visual
                .debug_bounds(selector("project-folder-all-", OTHER))
                .is_some()
        );
        // A chat in use stays listed, wherever it falls.
        window
            .update(visual, |shell, window, cx| shell.open(oldest, window, cx))
            .unwrap();
        visual.run_until_parked();
        assert!(
            visual
                .debug_bounds(selector("sidebar-session-", oldest))
                .is_some()
        );
    }

    #[gpui::test]
    fn the_all_row_opens_a_full_view_and_the_back_button_returns(cx: &mut TestAppContext) {
        let (window, home, old, _, oldest) = setup(cx);
        let ids = many_chats(window, cx, 7);
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();
        let all = visual
            .debug_bounds(selector("project-folder-all-", OTHER))
            .unwrap();
        visual.simulate_click(all.center(), Default::default());
        visual.run_until_parked();
        assert!(visual.debug_bounds("focused-project").is_some());
        assert!(visual.debug_bounds("project-list-back").is_some());
        assert!(
            visual
                .debug_bounds(selector("project-folder-", OTHER))
                .is_none()
        );
        assert!(
            visual
                .debug_bounds(selector("sidebar-session-", old))
                .is_none()
        );
        window
            .update(visual, |shell, _, cx| {
                assert_eq!(shell.workspace.project, OTHER);
                assert_eq!(shell.sidebar_session_ids(cx).len(), ids.len() + 1);
                // Session folders belong to this view.
                let menu = shell.session_menu_items(oldest, cx);
                assert!(menu.iter().any(|item| {
                    matches!(item, MenuItem::Action(action) if action.id == "folder-new")
                }));
                let saved = shell.latest_state.lock().unwrap().clone();
                let restored = Workspace::restore(Some(&saved), &[], OTHER);
                assert_eq!(restored.sidebar_focused_project.as_deref(), Some(OTHER));
            })
            .unwrap();
        let back = visual.debug_bounds("project-list-back").unwrap();
        visual.simulate_click(back.center(), Default::default());
        visual.run_until_parked();
        assert!(visual.debug_bounds("focused-project").is_none());
        assert!(
            visual
                .debug_bounds(selector("project-folder-", &home))
                .is_some()
        );
    }

    #[gpui::test]
    fn leaving_the_project_ends_its_full_view(cx: &mut TestAppContext) {
        let (window, _, old, _, _) = setup(cx);
        window
            .update(cx, |shell, _, _| {
                shell.workspace.project = OTHER.into();
                shell.workspace.sidebar_focused_project = Some(OTHER.into());
                assert!(!shell.project_list_showing());
            })
            .unwrap();
        window
            .update(cx, |shell, window, cx| {
                shell.open(old, window, cx);
                assert!(shell.project_list_showing());
                shell.end_stale_project_focus();
                assert_eq!(shell.workspace.sidebar_focused_project, None);
            })
            .unwrap();
    }

    #[gpui::test]
    fn a_long_popover_stops_at_its_limit_and_offers_the_full_view(cx: &mut TestAppContext) {
        let (window, _, _, _, oldest) = setup(cx);
        let ids = many_chats(window, cx, 12);
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();
        hover_folded(window, visual);
        assert!(
            visual
                .debug_bounds(selector("project-flyout-session-", ids[11]))
                .is_some()
        );
        assert!(
            visual
                .debug_bounds(selector("project-flyout-session-", oldest))
                .is_none()
        );
        let all = visual.debug_bounds("project-flyout-all").unwrap();
        visual.simulate_click(all.center(), Default::default());
        visual.run_until_parked();
        assert!(visual.debug_bounds("project-flyout").is_none());
        assert!(visual.debug_bounds("focused-project").is_some());
    }

    #[gpui::test]
    fn the_minus_button_closes_a_peeked_chat_back_into_its_folder(cx: &mut TestAppContext) {
        let (window, _, _, new, other) = setup(cx);
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();
        hover_folded(window, visual);
        let pick = visual
            .debug_bounds(selector("project-flyout-session-", other))
            .unwrap();
        visual.simulate_click(pick.center(), Default::default());
        visual.run_until_parked();
        // Chats in an open folder's normal list have no such button.
        window
            .update(visual, |shell, window, cx| shell.open(new, window, cx))
            .unwrap();
        visual.run_until_parked();
        assert!(
            visual
                .debug_bounds(selector("peeked-session-close-", new))
                .is_none()
        );
        let row = visual
            .debug_bounds(selector("sidebar-session-", other))
            .unwrap();
        visual.simulate_mouse_move(row.center(), None, Default::default());
        visual.run_until_parked();
        let close = visual
            .debug_bounds(selector("peeked-session-close-", other))
            .unwrap();
        visual.simulate_click(close.center(), Default::default());
        visual.run_until_parked();
        assert!(
            visual
                .debug_bounds(selector("sidebar-session-", other))
                .is_none()
        );
        window
            .update(visual, |shell, _, _| {
                assert!(shell.workspace.tab_of(other).is_none());
                assert_eq!(shell.workspace.active, Some(new));
                assert!(shell.workspace.collapsed.contains(OTHER));
            })
            .unwrap();
    }

    #[gpui::test]
    fn the_header_holds_a_wide_search_field(cx: &mut TestAppContext) {
        let (window, _, _, _, _) = setup(cx);
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();
        assert!(visual.debug_bounds("sidebar-search-button").is_none());
        let field = visual.debug_bounds("sidebar-search-field").unwrap();
        let options = visual.debug_bounds("sidebar-options").unwrap();
        assert!(field.right() <= options.left());
        assert!(field.size.width > px(120.));
        let label = visual.debug_bounds("sidebar-search-label").unwrap();
        assert!((label.center().y - field.center().y).abs() < px(1.));
        visual.simulate_mouse_down(field.center(), MouseButton::Left, Default::default());
        visual.run_until_parked();
        window
            .update(visual, |shell, _, _| assert!(shell.app_search.is_some()))
            .unwrap();
        // The full view of one project keeps its back button and the icon.
        window
            .update(visual, |shell, window, cx| {
                shell.close_app_search(window, cx);
                let project = shell.workspace.project.clone();
                shell.workspace.sidebar_focused_project = Some(project);
                shell.invalidate_chrome(cx);
            })
            .unwrap();
        visual.run_until_parked();
        assert!(visual.debug_bounds("sidebar-search-field").is_none());
        assert!(visual.debug_bounds("sidebar-search-button").is_some());
        assert!(visual.debug_bounds("project-list-back").is_some());
    }

    #[gpui::test]
    fn folding_hides_open_chats_except_the_one_in_use(cx: &mut TestAppContext) {
        let (window, home, old, new, _) = setup(cx);
        window
            .update(cx, |shell, window, cx| {
                shell.open(old, window, cx);
                shell.open(new, window, cx);
                assert!(shell.workspace.tab_of(old).is_some());
                assert_eq!(shell.workspace.active, Some(new));
            })
            .unwrap();
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();
        let row = visual
            .debug_bounds(selector("project-folder-", &home))
            .unwrap();
        visual.simulate_click(row.center(), Default::default());
        visual.run_until_parked();
        assert!(
            visual
                .debug_bounds(selector("sidebar-session-", new))
                .is_some()
        );
        assert!(
            visual
                .debug_bounds(selector("sidebar-session-", old))
                .is_none(),
            "a chat with an open tab still folds away"
        );
        window
            .update(visual, |shell, _, _| {
                assert!(shell.workspace.tab_of(old).is_some(), "folding keeps tabs");
            })
            .unwrap();
    }

    #[gpui::test]
    fn a_chat_preview_card_does_not_swallow_a_popover_pick(cx: &mut TestAppContext) {
        let (window, _, old, _, other) = setup(cx);
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();
        hover_folded(window, visual);
        window
            .update(visual, |shell, _, cx| {
                shell.session_preview = Some(SessionPreview {
                    id: old,
                    data: None,
                    anchor: AnchorSlot::default(),
                    surface: AnchorSlot::default(),
                });
                shell.sidebar_frame.update(cx, |_, cx| cx.notify());
            })
            .unwrap();
        visual.run_until_parked();
        let pick = visual
            .debug_bounds(selector("project-flyout-session-", other))
            .unwrap();
        visual.simulate_click(pick.center(), Default::default());
        visual.run_until_parked();
        window
            .update(visual, |shell, _, _| {
                assert_eq!(shell.workspace.active, Some(other));
                assert!(shell.session_preview.is_none());
            })
            .unwrap();
        assert!(
            visual
                .debug_bounds(selector("sidebar-session-", other))
                .is_some()
        );
    }

    #[gpui::test]
    fn a_new_chat_names_its_project_and_offers_the_project_picker(cx: &mut TestAppContext) {
        let (window, _, _, _, _) = setup(cx);
        window
            .update(cx, |shell, _, cx| shell.new_chat(cx))
            .unwrap();
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();
        let chip = visual.debug_bounds("empty-session-project").unwrap();
        visual.simulate_click(chip.center(), Default::default());
        visual.run_until_parked();
        window
            .update(visual, |shell, _, _| {
                assert!(shell.menu_open(&MenuTarget::CompactProjectPicker));
            })
            .unwrap();
    }

    fn rail_window(cx: &mut TestAppContext) -> (gpui::WindowHandle<Shell>, i64) {
        cx.update(|cx| cx.set_reduce_motion(true));
        let (window, _, _, _, other) = setup(cx);
        window
            .update(cx, |shell, _, cx| {
                shell.workspace.preferences.swap_side_panels = true;
                shell.set_sidebar_visible(false, cx);
                shell.layout_changed(cx);
            })
            .unwrap();
        cx.run_until_parked();
        (window, other)
    }

    #[gpui::test]
    fn a_collapsed_sidebar_leaves_project_icons_with_a_popover(cx: &mut TestAppContext) {
        let (window, other) = rail_window(cx);
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();
        let rail = visual.debug_bounds("project-icon-rail").unwrap();
        assert_eq!(rail.size.width, px(PROJECT_RAIL_WIDTH));
        let main = visual.debug_bounds("main-pane-frame").unwrap();
        assert!(main.right() <= rail.left() + px(1.));
        let icon = visual
            .debug_bounds(selector("project-rail-", OTHER))
            .unwrap();
        visual.simulate_mouse_move(icon.center(), None, Default::default());
        visual.run_until_parked();
        visual.executor().advance_clock(FLYOUT_OPEN_DELAY);
        visual.run_until_parked();
        assert!(visual.debug_bounds("project-flyout-name").is_some());
        let pick = visual
            .debug_bounds(selector("project-flyout-session-", other))
            .unwrap();
        assert!(
            pick.right() <= icon.left(),
            "the popover opens toward the middle"
        );
        visual.simulate_click(pick.center(), Default::default());
        visual.run_until_parked();
        window
            .update(visual, |shell, _, _| {
                assert_eq!(shell.workspace.active, Some(other));
                assert_eq!(shell.workspace.project, OTHER);
                assert!(!shell.workspace.sidebar_visible);
            })
            .unwrap();
    }

    #[gpui::test]
    fn the_popover_closes_after_passing_over_another_icon(cx: &mut TestAppContext) {
        let (window, _) = rail_window(cx);
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();
        let (home, far) = window
            .update(visual, |shell, _, _| {
                (
                    shell.workspace.projects[0].clone(),
                    gpui::point(px(500.), px(500.)),
                )
            })
            .unwrap();
        let other = visual
            .debug_bounds(selector("project-rail-", OTHER))
            .unwrap();
        let first = visual
            .debug_bounds(selector("project-rail-", &home))
            .unwrap();
        visual.simulate_mouse_move(other.center(), None, Default::default());
        visual.run_until_parked();
        visual.executor().advance_clock(FLYOUT_OPEN_DELAY);
        visual.run_until_parked();
        assert!(visual.debug_bounds("project-flyout").is_some());
        // Across another icon and out, faster than a popover can open there.
        visual.simulate_mouse_move(first.center(), None, Default::default());
        visual.run_until_parked();
        visual.simulate_mouse_move(far, None, Default::default());
        visual.run_until_parked();
        visual.executor().advance_clock(FLYOUT_CLOSE_DELAY);
        visual.run_until_parked();
        assert!(visual.debug_bounds("project-flyout").is_none());
    }

    #[gpui::test]
    fn the_popover_closes_even_if_the_hover_event_never_comes(cx: &mut TestAppContext) {
        let (window, _) = rail_window(cx);
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();
        let icon = visual
            .debug_bounds(selector("project-rail-", OTHER))
            .unwrap();
        visual.simulate_mouse_move(icon.center(), None, Default::default());
        visual.run_until_parked();
        visual.executor().advance_clock(FLYOUT_OPEN_DELAY);
        visual.run_until_parked();
        assert!(visual.debug_bounds("project-flyout").is_some());
        // The slot goes stale, as if the pointer had left unnoticed.
        window
            .update(visual, |shell, _, _| {
                shell
                    .project_flyout
                    .rows
                    .insert(OTHER.into(), AnchorSlot::default());
                shell.project_flyout.surface = AnchorSlot::default();
            })
            .unwrap();
        visual
            .executor()
            .advance_clock(FLYOUT_WATCH_INTERVAL * FLYOUT_WATCH_MISSES);
        visual.run_until_parked();
        window
            .update(visual, |shell, _, _| {
                assert!(shell.project_flyout.open.is_none())
            })
            .unwrap();
    }

    #[gpui::test]
    fn the_icon_strip_goes_away_when_the_sidebar_opens(cx: &mut TestAppContext) {
        let (window, _) = rail_window(cx);
        window
            .update(cx, |shell, _, cx| {
                shell.set_sidebar_visible(true, cx);
                cx.notify();
            })
            .unwrap();
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();
        assert!(visual.debug_bounds("project-icon-rail").is_none());
    }

    fn open_file(shell: &mut Shell, window: &mut Window, cx: &mut Context<Shell>) {
        shell.open_review(
            crate::review::ReviewTab::File {
                cwd: shell.workspace.project.clone(),
                path: "/tmp/shared-file.rs".into(),
            },
            window,
            cx,
        );
    }

    #[gpui::test]
    fn open_files_follow_you_between_chats_on_the_same_folder(cx: &mut TestAppContext) {
        cx.update(|cx| cx.set_reduce_motion(true));
        let (window, _, old, new, _) = setup(cx);
        window
            .update(cx, |shell, w, cx| {
                shell.open(old, w, cx);
                open_file(shell, w, cx);
                let review = shell.reviews[&old].clone();
                review.update(cx, |r, _| r.state.ratio = 0.62);
                shell.open(new, w, cx);
                assert_eq!(shell.reviews[&new], review, "the same open editor moves");
                assert!(!shell.reviews.contains_key(&old));
                assert_eq!(shell.workspace.reviews[&new].ratio, 0.62);
                assert!(!shell.workspace.reviews.contains_key(&old));
                shell.open(old, w, cx);
                assert_eq!(shell.reviews[&old], review, "and comes back");
            })
            .unwrap();
    }

    #[gpui::test]
    fn a_chat_with_its_own_files_keeps_them_at_the_same_width(cx: &mut TestAppContext) {
        cx.update(|cx| cx.set_reduce_motion(true));
        let (window, _, old, new, _) = setup(cx);
        window
            .update(cx, |shell, w, cx| {
                shell.open(new, w, cx);
                open_file(shell, w, cx);
                let own = shell.reviews[&new].clone();
                shell.workspace.sidebar_project_folders = false;
                shell.open(old, w, cx);
                open_file(shell, w, cx);
                shell.workspace.sidebar_project_folders = true;
                shell.reviews[&old].update(cx, |r, _| r.state.ratio = 0.7);
                shell.open(new, w, cx);
                assert_eq!(shell.reviews[&new], own);
                assert!(shell.reviews.contains_key(&old));
                assert_eq!(own.read(cx).state.ratio, 0.7);
            })
            .unwrap();
    }

    #[gpui::test]
    fn open_files_stay_behind_when_the_folder_changes(cx: &mut TestAppContext) {
        cx.update(|cx| cx.set_reduce_motion(true));
        let (window, _, old, _, other) = setup(cx);
        window
            .update(cx, |shell, w, cx| {
                shell.open(old, w, cx);
                open_file(shell, w, cx);
                shell.open(other, w, cx);
                assert!(shell.reviews.contains_key(&old));
                assert!(!shell.reviews.contains_key(&other));
            })
            .unwrap();
    }

    #[gpui::test]
    fn the_popover_starts_a_new_chat_in_the_folded_project(cx: &mut TestAppContext) {
        let (window, _, _, _, other) = setup(cx);
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();
        hover_folded(window, visual);
        let new = visual.debug_bounds("project-flyout-new").unwrap();
        visual.simulate_click(new.center(), Default::default());
        visual.run_until_parked();
        window
            .update(visual, |shell, _, cx| {
                assert_eq!(shell.workspace.project, OTHER);
                assert!(shell.workspace.collapsed.contains(OTHER));
                let active = shell.workspace.active.unwrap();
                assert_ne!(active, other);
                assert_eq!(shell.session_metadata(active, cx).summary.cwd, OTHER);
            })
            .unwrap();
    }

    #[gpui::test]
    fn session_folder_actions_stay_out_of_the_project_list(cx: &mut TestAppContext) {
        let (window, _, old, new, other) = setup(cx);
        window
            .update(cx, |shell, _, cx| {
                let menu = shell.session_menu_items(other, cx);
                assert!(!menu.iter().any(|item| {
                    matches!(item, MenuItem::Action(action) if action.id.starts_with("folder"))
                }));
                assert!(shell.sidebar_drop_at(Point::default(), old, cx).is_none());
                let ids = shell.sidebar_navigation_ids(cx);
                assert!(ids.ends_with(&[new, old, other]));
                // A chat chosen in another project still gets its own menu.
                shell.selected_sessions.insert(other);
                assert_eq!(shell.session_menu_ids(other, cx), [other]);
            })
            .unwrap();
    }
}
