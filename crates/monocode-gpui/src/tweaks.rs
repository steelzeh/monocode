//! The tweaks the user installed, and the panel that shows them. The rules for
//! what a tweak may do live in `monocode_core::tweaks`.
use crate::{
    runtime::{Database, locked},
    settings::widgets::secondary_button,
    theme::*,
    visuals::{TweakActions, VisualView},
};
use gpui::{
    App, AssetSource, Context, Entity, EventEmitter, Global, SharedString, Subscription, Window,
    div, prelude::*, px,
};
use monocode_core::{
    tweaks::{self, AppCall, Context as PanelContext, Found, Grants, Guard, Origin, Tweak},
    visual::{VisualDocument, VisualRuntime},
};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
};

struct Shared(Entity<Tweaks>);
impl Global for Shared {}
pub(crate) fn get(cx: &App) -> Option<Entity<Tweaks>> {
    cx.try_global::<Shared>().map(|shared| shared.0.clone())
}
pub(crate) fn install(store: Database, cx: &mut App) {
    if get(cx).is_some() {
        return;
    }
    let (root, grants) = locked(&store)
        .map(|store| {
            (
                store.data_dir().map(|dir| dir.join("tweaks")),
                Grants::parse(store.setting(Grants::setting()).ok().flatten().as_deref()),
            )
        })
        .unwrap_or_default();
    let center = cx.new(|cx| {
        let mut center = Tweaks {
            root: root.unwrap_or_default(),
            found: Vec::new(),
            grants,
            store,
            revision: 0,
            scripts: HashMap::new(),
            next_generation: 0,
        };
        center.reload(cx);
        center
    });
    cx.set_global(Shared(center));
}

pub(crate) struct Tweaks {
    root: PathBuf,
    found: Vec<Found>,
    grants: Grants,
    store: Database,
    revision: u64,
    scripts: HashMap<String, Script>,
    next_generation: u64,
}
/// A tweak's running script. Its calls end when the script is dropped.
struct Script {
    runtime: VisualRuntime,
    guard: Guard,
    /// Tells this start of the script from any earlier one.
    generation: u64,
    cancel: Arc<AtomicBool>,
}
impl Drop for Script {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}
/// The app calls a script made, and what they need to report back.
#[derive(Default)]
pub(crate) struct ScriptRun {
    pub calls: Vec<AppCall>,
    pub generation: u64,
    pub cancel: Arc<AtomicBool>,
}
/// A menu item whose click the tweak's own script handles.
pub(crate) struct MenuScript {
    pub tweak: String,
    pub entry: String,
}
pub(crate) struct MenuItem {
    pub key: String,
    pub tweak: String,
    pub label: String,
    pub busy_label: Option<String>,
    /// A prompt goes to the agent, so it waits for the agent to finish.
    pub needs_idle: bool,
}
impl Tweaks {
    pub(crate) fn root(&self) -> &Path {
        &self.root
    }
    pub(crate) fn found(&self) -> &[Found] {
        &self.found
    }
    pub(crate) fn revision(&self) -> u64 {
        self.revision
    }
    pub(crate) fn is_on(&self, tweak: &Tweak) -> bool {
        self.grants.allows(tweak)
    }
    /// Was turned on, but its files changed since. It stays off until accepted.
    pub(crate) fn is_changed(&self, tweak: &Tweak) -> bool {
        self.grants.changed(tweak)
    }
    fn on(&self) -> impl Iterator<Item = &Tweak> {
        self.found.iter().filter_map(|found| match found {
            Found::Ready(tweak) if self.is_on(tweak) => Some(tweak),
            _ => None,
        })
    }
    pub(crate) fn has_panels(&self) -> bool {
        self.on().any(|tweak| tweak.panel_source.is_some())
    }
    pub(crate) fn reload(&mut self, cx: &mut Context<Self>) {
        let root = self.root.clone();
        let work = cx
            .background_executor()
            .spawn(async move { tweaks::discover(&root) });
        cx.spawn(async move |this, cx| {
            let found = work.await;
            let _ = this.update(cx, |this, cx| {
                this.found = found;
                this.revision += 1;
                this.scripts.clear();
                cx.notify();
            });
        })
        .detach();
    }
    pub(crate) fn set_enabled(&mut self, tweak: &Tweak, enabled: bool, cx: &mut Context<Self>) {
        self.grants.set(tweak, enabled);
        if let Ok(store) = locked(&self.store) {
            let _ = store.save_setting(Grants::setting(), &self.grants.encode());
        }
        self.revision += 1;
        self.scripts.remove(&tweak.manifest.id);
        cx.notify();
    }
    /// Runs the script's handler for a menu click, starting the script if needed.
    pub(crate) fn run_menu(
        &mut self,
        id: &str,
        entry: &str,
        chat: &str,
        payload: &str,
    ) -> Result<ScriptRun, String> {
        if !self.scripts.contains_key(id) {
            let tweak = self
                .on()
                .find(|tweak| tweak.manifest.id == id)
                .ok_or("This tweak is off")?;
            let source = tweak
                .script_source
                .clone()
                .ok_or("This tweak has no script")?;
            let guard = Guard::new(&tweak.manifest.permissions);
            let runtime = VisualRuntime::tweak_script(&source).map_err(friendly)?;
            self.next_generation += 1;
            self.scripts.insert(
                id.to_owned(),
                Script {
                    runtime,
                    guard,
                    generation: self.next_generation,
                    cancel: Arc::default(),
                },
            );
        }
        let script = self.scripts.get_mut(id).ok_or("This tweak is off")?;
        script.guard.allow_chat(chat);
        let origin = script.guard.click();
        let result = script
            .runtime
            .run_menu(entry, payload)
            .and_then(|()| script.runtime.take_calls());
        self.finish(id, origin, result)
    }
    /// Continues a script with an answer. An answer for a script that has
    /// since stopped or restarted has no one left to tell.
    pub(crate) fn deliver(
        &mut self,
        id: &str,
        generation: u64,
        request: u64,
        payload: &str,
    ) -> Result<ScriptRun, String> {
        let Some(script) = self
            .scripts
            .get_mut(id)
            .filter(|script| script.generation == generation)
        else {
            return Ok(ScriptRun::default());
        };
        let Some(origin) = script.guard.answered(request) else {
            return Ok(ScriptRun::default());
        };
        let result = script
            .runtime
            .deliver(request, payload)
            .and_then(|()| script.runtime.take_calls());
        self.finish(id, origin, result)
    }
    /// A failed script is dropped and starts fresh next time.
    fn finish(
        &mut self,
        id: &str,
        origin: Origin,
        result: Result<Vec<AppCall>, String>,
    ) -> Result<ScriptRun, String> {
        let calls = match result {
            Ok(calls) => calls,
            Err(error) => {
                self.scripts.remove(id);
                return Err(friendly(error));
            }
        };
        let name = self
            .on()
            .find(|tweak| tweak.manifest.id == id)
            .map(|tweak| tweak.manifest.name.clone())
            .unwrap_or_default();
        let Some(script) = self.scripts.get_mut(id) else {
            return Ok(ScriptRun::default());
        };
        match script.guard.admit(origin, calls) {
            Ok(calls) => Ok(ScriptRun {
                calls,
                generation: script.generation,
                cancel: script.cancel.clone(),
            }),
            Err(error) => {
                self.scripts.remove(id);
                Err(format!("{name} {error}, so it was stopped."))
            }
        }
    }
    #[cfg(test)]
    pub(crate) fn forget_first_read(&mut self) {
        self.revision = 0;
    }
    #[cfg(test)]
    pub(crate) fn set_root(&mut self, root: PathBuf) {
        self.root = root;
    }
    #[cfg(test)]
    pub(crate) fn set_found(&mut self, found: Vec<Found>, cx: &mut Context<Self>) {
        self.found = found;
        self.revision += 1;
        cx.notify();
    }
}

fn friendly(error: String) -> String {
    error.replace("Visual execution failed: ", "")
}

/// Names that the app or a slash command of its own already answers to.
fn reserved(name: &str) -> bool {
    crate::input::Command::from_name(name).is_some()
        || matches!(
            name,
            "compact" | "btw" | "stage" | "commit" | "visualize" | "mcp" | "add-to-folder"
        )
}

/// The enabled tweaks' slash commands: name, description.
pub(crate) fn commands(cx: &App) -> Vec<(String, String)> {
    let Some(center) = get(cx) else {
        return Vec::new();
    };
    let center = center.read(cx);
    let mut seen = std::collections::HashSet::new();
    center
        .on()
        .flat_map(|tweak| &tweak.manifest.commands)
        .filter(|command| !reserved(&command.name) && seen.insert(command.name.clone()))
        .map(|command| (command.name.clone(), command.description.clone()))
        .collect()
}

/// What to tell the agent for a typed `/command`, when a tweak owns it.
pub(crate) fn command_prompt(text: &str, cx: &App) -> Option<String> {
    let rest = text.trim_start().strip_prefix('/')?;
    let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
    let (name, args) = rest.split_at(end);
    if reserved(&name.to_ascii_lowercase()) {
        return None;
    }
    let center = get(cx)?;
    let center = center.read(cx);
    let command = center
        .on()
        .flat_map(|tweak| &tweak.manifest.commands)
        .find(|command| command.name.eq_ignore_ascii_case(name))?;
    Some(tweaks::expand_prompt(&command.prompt, args))
}

/// The enabled tweaks' chat menu items.
pub(crate) fn menu_items(cx: &App) -> Vec<MenuItem> {
    let Some(center) = get(cx) else {
        return Vec::new();
    };
    let center = center.read(cx);
    center
        .on()
        .flat_map(|tweak| {
            tweak.manifest.menu.iter().map(|entry| MenuItem {
                key: format!("tweak:{}:{}", tweak.manifest.id, entry.id),
                tweak: tweak.manifest.id.clone(),
                label: entry.label.clone(),
                busy_label: entry.busy_label.clone(),
                needs_idle: entry.prompt.is_some(),
            })
        })
        .collect()
}

fn menu_entry<'a>(center: &'a Tweaks, action: &str) -> Option<(&'a Tweak, &'a tweaks::MenuEntry)> {
    let (id, entry) = action.strip_prefix("tweak:")?.split_once(':')?;
    let tweak = center.on().find(|tweak| tweak.manifest.id == id)?;
    Some((
        tweak,
        tweak.manifest.menu.iter().find(|item| item.id == entry)?,
    ))
}

/// A menu action's label and the prompt it sends, when it is a prompt item.
pub(crate) fn menu_prompt(action: &str, cx: &App) -> Option<(String, String)> {
    let center = get(cx)?;
    let center = center.read(cx);
    let (_, entry) = menu_entry(center, action)?;
    Some((entry.label.clone(), entry.prompt.clone()?))
}

/// The script that handles a menu action, when it is not a prompt item.
pub(crate) fn menu_script(action: &str, cx: &App) -> Option<MenuScript> {
    let center = get(cx)?;
    let center = center.read(cx);
    let (tweak, entry) = menu_entry(center, action)?;
    entry.prompt.is_none().then(|| MenuScript {
        tweak: tweak.manifest.id.clone(),
        entry: entry.id.clone(),
    })
}

/// The app calls a panel made, and what they need to report back.
pub(crate) struct PanelRun {
    pub tweak: String,
    pub generation: u64,
    pub cancel: Arc<AtomicBool>,
    pub calls: Vec<AppCall>,
}

pub(crate) enum TweakPanelEvent {
    Calls(PanelRun),
    /// The panel on show changed, so its tab has a new name.
    Selected,
}

/// The Tweaks tab's name: the title of the panel on show, as the tweak set it.
pub(crate) fn tab_title(view: Option<&Entity<TweakPanel>>, cx: &App) -> SharedString {
    shown_panel(view, cx).map_or_else(|| "Tweaks".into(), |panel| panel.title.into())
}

/// The Tweaks tab's icon: the one its panel names, when the app has it.
pub(crate) fn tab_icon(view: Option<&Entity<TweakPanel>>, cx: &App) -> &'static str {
    shown_panel(view, cx)
        .and_then(|panel| shipped_icon(panel.icon.as_deref()?))
        .unwrap_or("command-visualize")
}

fn shown_panel(view: Option<&Entity<TweakPanel>>, cx: &App) -> Option<tweaks::Panel> {
    let selected = view.and_then(|view| view.read(cx).selected.clone());
    let panels = TweakPanel::panels(cx);
    panels
        .iter()
        .find(|tweak| Some(&tweak.manifest.id) == selected.as_ref())
        .or(panels.first())
        .and_then(|tweak| tweak.manifest.panel.clone())
}

/// The app's own name for an icon. Only names the app ships are kept, so this
/// holds at most one short string per icon for the life of the app.
fn shipped_icon(name: &str) -> Option<&'static str> {
    static NAMES: OnceLock<Mutex<HashMap<String, &'static str>>> = OnceLock::new();
    if !matches!(crate::assets::Assets.load(name), Ok(Some(_))) {
        return None;
    }
    let mut names = NAMES.get_or_init(Default::default).lock().ok()?;
    let known = names
        .entry(name.to_owned())
        .or_insert_with(|| Box::leak(name.to_owned().into_boxed_str()));
    Some(*known)
}

/// The right panel's Tweaks tab: one enabled tweak's panel at a time.
pub(crate) struct TweakPanel {
    selected: Option<String>,
    loaded: Option<(String, String)>,
    view: Option<Entity<VisualView>>,
    context: PanelContext,
    guard: Guard,
    generation: u64,
    cancel: Arc<AtomicBool>,
    _view_events: Option<Subscription>,
    _center: Option<Subscription>,
}
impl EventEmitter<TweakPanelEvent> for TweakPanel {}

impl TweakPanel {
    pub(crate) fn new(cx: &mut Context<Self>) -> Self {
        Self {
            selected: None,
            loaded: None,
            view: None,
            context: PanelContext::default(),
            guard: Guard::default(),
            generation: 0,
            cancel: Arc::default(),
            _view_events: None,
            _center: get(cx).map(|center| cx.observe(&center, |this, _, cx| this.refresh(cx))),
        }
    }
    #[cfg(test)]
    pub(crate) fn view_for_test(&self) -> Option<Entity<VisualView>> {
        self.view.clone()
    }
    pub(crate) fn set_context(&mut self, context: PanelContext, cx: &mut Context<Self>) {
        self.context = context;
        self.refresh(cx);
    }
    pub(crate) fn select(&mut self, id: String, cx: &mut Context<Self>) {
        self.selected = Some(id);
        self.refresh(cx);
        cx.emit(TweakPanelEvent::Selected);
    }
    fn panels(cx: &App) -> Vec<Tweak> {
        get(cx)
            .map(|center| {
                center
                    .read(cx)
                    .on()
                    .filter(|tweak| tweak.panel_source.is_some())
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }
    /// Shows the selected tweak: its script again only when the script changed.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        let panels = Self::panels(cx);
        let tweak = panels
            .iter()
            .find(|tweak| Some(&tweak.manifest.id) == self.selected.as_ref())
            .or(panels.first());
        let Some(tweak) = tweak else {
            self.selected = None;
            self.loaded = None;
            self.view = None;
            self._view_events = None;
            cx.notify();
            return;
        };
        self.selected = Some(tweak.manifest.id.clone());
        let source = tweak.panel_source.clone().unwrap_or_default();
        let key = (tweak.manifest.id.clone(), source.clone());
        let context = serde_json::to_string(&self.context).unwrap_or_else(|_| "{}".into());
        if self.loaded.as_ref() == Some(&key)
            && let Some(view) = self
                .view
                .as_ref()
                .filter(|view| !view.read(cx).is_stopped())
        {
            view.update(cx, |view, cx| view.set_tweak_context(&context, cx));
        } else {
            let document = VisualDocument {
                id: tweak.manifest.id.clone(),
                title: tweak.manifest.name.clone(),
                source,
            };
            let view = cx.new(|cx| VisualView::tweak(document, context, cx));
            self.guard = Guard::new(&tweak.manifest.permissions);
            self.generation += 1;
            self.cancel.store(true, Ordering::Relaxed);
            self.cancel = Arc::default();
            self._view_events = Some(cx.subscribe(&view, |this, _, event: &TweakActions, cx| {
                let origin = this.guard.click();
                this.admit(origin, event.0.clone(), cx);
            }));
            self.view = Some(view);
            self.loaded = Some(key);
        }
        if let Some(view) = self.view.clone() {
            let origin = self.guard.refresh();
            let calls = view.update(cx, |view, cx| view.tweak_refresh(cx));
            self.admit(origin, calls, cx);
        }
        cx.notify();
    }
    /// Gives the panel an answer. One for an earlier start of it is ignored.
    pub(crate) fn deliver(
        &mut self,
        generation: u64,
        request: u64,
        payload: &str,
        cx: &mut Context<Self>,
    ) {
        if generation != self.generation {
            return;
        }
        let Some(origin) = self.guard.answered(request) else {
            return;
        };
        let Some(view) = self.view.clone() else {
            return;
        };
        let calls = view.update(cx, |view, cx| view.tweak_deliver(request, payload, cx));
        self.admit(origin, calls, cx);
    }
    /// Passes on the calls the guard allows, or stops the panel and says why.
    fn admit(&mut self, origin: Origin, calls: Vec<AppCall>, cx: &mut Context<Self>) {
        if calls.is_empty() {
            return;
        }
        let Some((tweak, name)) = self.loaded_tweak(cx) else {
            return;
        };
        match self.guard.admit(origin, calls) {
            Ok(calls) => cx.emit(TweakPanelEvent::Calls(PanelRun {
                tweak,
                generation: self.generation,
                cancel: self.cancel.clone(),
                calls,
            })),
            Err(error) => {
                if let Some(view) = &self.view {
                    let message = format!("{name} {error}, so it was stopped.");
                    view.update(cx, |view, cx| view.stop_tweak(message, cx));
                }
            }
        }
    }
    fn loaded_tweak(&self, cx: &App) -> Option<(String, String)> {
        let id = self.loaded.as_ref()?.0.clone();
        let center = get(cx)?;
        let name = center
            .read(cx)
            .on()
            .find(|tweak| tweak.manifest.id == id)
            .map_or_else(|| id.clone(), |tweak| tweak.manifest.name.clone());
        Some((id, name))
    }
}

impl Render for TweakPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let panels = Self::panels(cx);
        let selected = self.selected.clone();
        div()
            .id("tweak-panel")
            .debug_selector(|| "tweak-panel".into())
            .flex_1()
            .min_h_0()
            .w_full()
            .flex()
            .flex_col()
            .when(panels.len() > 1, |d| {
                d.child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap_1()
                        .p_2()
                        .border_b_1()
                        .border_color(content(0.08))
                        .children(panels.iter().map(|tweak| {
                            let id = tweak.manifest.id.clone();
                            let active = selected.as_ref() == Some(&id);
                            let title = tweak
                                .manifest
                                .panel
                                .as_ref()
                                .map_or(tweak.manifest.name.clone(), |p| p.title.clone());
                            div()
                                .id(format!("tweak-panel-tab-{id}"))
                                .debug_selector({
                                    let id = id.clone();
                                    move || format!("tweak-panel-tab-{id}")
                                })
                                .px_2()
                                .py_1()
                                .rounded_md()
                                .cursor_pointer()
                                .text_size(px(12.))
                                .when(active, |d| d.bg(content(0.08)))
                                .text_color(content(if active { 1. } else { 0.6 }))
                                .child(title)
                                .on_click(
                                    cx.listener(move |this, _, _, cx| this.select(id.clone(), cx)),
                                )
                        })),
                )
            })
            .child(
                div()
                    .id("tweak-panel-body")
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .overflow_y_scroll()
                    .p_3()
                    .children(self.view.clone()),
            )
    }
}

/// A button that opens the tweaks folder, made when it is first needed.
pub(crate) fn open_folder_button(root: PathBuf) -> impl IntoElement {
    secondary_button("tweaks-open-folder", "Open folder", false, false)
        .child(icon("folder", 14.))
        .child("Open folder")
        .on_click(move |_, _, _| {
            let _ = std::fs::create_dir_all(&root);
            let _ = monocode_core::files::reveal_path(&root);
        })
}
