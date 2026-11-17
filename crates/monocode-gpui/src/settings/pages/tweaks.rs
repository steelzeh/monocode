//! Settings → Tweaks: what is installed, what each can do, and its switch.
use crate::input::{self, Input};
use crate::settings::{
    SettingsView,
    widgets::{primary_button, red, secondary_button},
};
use crate::theme::*;
use crate::tweaks;
use gpui::{
    AnyElement, Context, Entity, FontWeight, Hsla, IntoElement, SharedString, Task, div,
    prelude::*, px,
};
use monocode_core::app_access::tweak_permission;
use monocode_core::tweaks::{self as core, Fetched, Found, Manifest, Source, Tweak};
use std::collections::HashSet;

/// The Add from GitHub form, and which rows have their prompts open.
#[derive(Default)]
pub(crate) struct TweakInstall {
    open: bool,
    link: Option<Entity<Input>>,
    stage: Stage,
    task: Option<Task<()>>,
    shown_prompts: HashSet<String>,
}

#[derive(Default)]
enum Stage {
    #[default]
    Idle,
    Looking,
    Preview(Box<Fetched>),
    Installing(Box<Fetched>),
    Failed(String),
}

/// The warning colour charts use, for the one permission that spends tokens.
fn cost_color() -> Hsla {
    crate::visuals::color(&monocode_core::visual::ThemeColor::Warning)
}

/// The small text beside a tweak's name.
pub(in crate::settings) fn meta_line(manifest: &Manifest, source: Option<&Source>) -> String {
    let version = manifest.version.trim().trim_start_matches('v');
    let mut parts = vec![format!("v{version}")];
    if let Some(author) = manifest
        .author
        .as_deref()
        .map(str::trim)
        .filter(|author| !author.is_empty())
    {
        parts.push(format!("by {author}"));
    }
    if let Some(source) = source {
        parts.push(format!("{} at {}", source.repo, source.short_commit()));
    }
    parts.join(" · ")
}

/// A place in the app, named only when the tweak calls it something else.
fn place(kind: &str, label: &str, tweak: &str) -> String {
    if label.eq_ignore_ascii_case(tweak) {
        kind.to_owned()
    } else {
        format!("{kind}: {label}")
    }
}

/// What turning a tweak on adds to the app: an icon and a label each.
pub(in crate::settings) fn adds_labels(manifest: &Manifest) -> Vec<(&'static str, String)> {
    let mut labels = Vec::new();
    if let Some(panel) = &manifest.panel {
        labels.push((
            "panel-right",
            place("Side panel", &panel.title, &manifest.name),
        ));
    }
    labels.extend(
        manifest
            .commands
            .iter()
            .map(|command| ("terminal", format!("/{}", command.name))),
    );
    labels.extend(manifest.menu.iter().map(|entry| {
        (
            "list-bullet",
            place("Chat menu", &entry.label, &manifest.name),
        )
    }));
    labels
}

/// One thing a tweak adds or may do: a small icon and its words, no box.
fn capability(selector: String, glyph: &'static str, tint: Hsla, text: String) -> AnyElement {
    div()
        .debug_selector(move || selector.clone())
        .flex()
        .items_center()
        .gap(px(6.))
        .text_size(px(12.))
        .line_height(px(18.))
        .text_color(content(0.62))
        .child(icon(glyph, 13.).text_color(tint))
        .child(text)
        .into_any_element()
}

/// One quiet line: what appears in the app, a hairline, what the tweak may do.
fn details(manifest: &Manifest) -> AnyElement {
    let id = &manifest.id;
    let adds: Vec<AnyElement> = adds_labels(manifest)
        .into_iter()
        .enumerate()
        .map(|(ix, (glyph, label))| {
            capability(
                format!("settings-tweak-adds-{id}-{ix}"),
                glyph,
                content(0.4),
                label,
            )
        })
        .collect();
    let permissions: Vec<AnyElement> = if manifest.permissions.is_empty() {
        vec![capability(
            format!("settings-tweak-permission-{id}-none"),
            "check-circle",
            content(0.4),
            "No permissions".to_owned(),
        )]
    } else {
        manifest
            .permissions
            .iter()
            .enumerate()
            .map(|(ix, name)| {
                let info = tweak_permission(name);
                // Only these reach a model, so only they spend your tokens.
                let tint = if info.is_some_and(|info| info.spends_tokens) {
                    cost_color()
                } else {
                    content(0.4)
                };
                capability(
                    format!("settings-tweak-permission-{id}-{ix}"),
                    info.map_or("command-visualize", |info| info.icon),
                    tint,
                    info.map_or_else(|| name.clone(), |info| info.label.to_owned()),
                )
            })
            .collect()
    };
    let has_adds = !adds.is_empty();
    div()
        .mt(px(10.))
        .flex()
        .flex_wrap()
        .items_center()
        .gap_x(px(14.))
        .gap_y(px(4.))
        .children(adds)
        .when(has_adds, |d| {
            d.child(div().w(px(1.)).h(px(12.)).bg(content(0.12)))
        })
        .children(permissions)
        .into_any_element()
}

fn title(manifest: &Manifest, source: Option<&Source>) -> impl IntoElement {
    let id = manifest.id.clone();
    div()
        .min_w_0()
        .flex()
        .items_baseline()
        .gap_2()
        .child(manifest.name.clone())
        .child(
            div()
                .debug_selector(move || format!("settings-tweak-meta-{id}"))
                .flex_shrink_0()
                .text_size(px(12.))
                .font_weight(FontWeight::NORMAL)
                .text_color(content(0.4))
                .child(meta_line(manifest, source)),
        )
}

/// What installing does to a tweak or folder that already has this id.
pub(in crate::settings) fn replace_note(found: &[Found], id: &str) -> String {
    found
        .iter()
        .find_map(|found| match found {
            Found::Ready(tweak) if tweak.manifest.id == id => Some(format!(
                "This replaces v{}, deletes its folder, and stays off until you turn it on.",
                tweak.manifest.version.trim().trim_start_matches('v')
            )),
            Found::Broken { folder, .. } if folder == id => Some(format!(
                "This replaces the broken folder {id}, deletes it, and stays off until you turn it on."
            )),
            _ => None,
        })
        .unwrap_or_else(|| "It stays off until you turn it on.".to_owned())
}

/// Every prompt a tweak can send: what it is called, and its full text.
fn prompts(manifest: &Manifest) -> Vec<(String, String)> {
    manifest
        .commands
        .iter()
        .map(|command| (format!("/{}", command.name), command.prompt.clone()))
        .chain(manifest.menu.iter().filter_map(|entry| {
            entry
                .prompt
                .clone()
                .map(|prompt| (entry.label.clone(), prompt))
        }))
        .collect()
}

/// The prompts in full, set back from the row like quoted text.
fn prompt_list(prefix: &str, items: Vec<(String, String)>) -> AnyElement {
    let prefix = prefix.to_owned();
    div()
        .mt(px(10.))
        .w_full()
        .flex()
        .flex_col()
        .gap_3()
        .p_3()
        .rounded_lg()
        .bg(content(0.03))
        .children(items.into_iter().enumerate().map(|(ix, (name, prompt))| {
            let selector = format!("{prefix}-{ix}");
            div()
                .debug_selector(move || selector.clone())
                .flex()
                .flex_col()
                .gap_1()
                .text_size(px(12.))
                .line_height(px(18.))
                .child(
                    div()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(content(0.75))
                        .child(name),
                )
                .child(div().text_color(content(0.55)).child(prompt))
        }))
        .into_any_element()
}

impl SettingsView {
    pub(super) fn tweaks_page(&mut self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let Some(center) = tweaks::get(cx) else {
            return Vec::new();
        };
        let (found, root) = {
            let center = center.read(cx);
            (center.found().to_vec(), center.root().to_path_buf())
        };
        let mut rows = Vec::new();
        for (ix, item) in found.iter().enumerate() {
            match item {
                Found::Ready(tweak) => rows.push(self.tweak_row(ix == 0, tweak, &center, cx)),
                Found::Broken { folder, error } => {
                    rows.push(self.row(
                        None,
                        ix == 0,
                        folder.clone(),
                        Some(format!("Could not load: {error}").into()),
                        Vec::new(),
                    ));
                }
            }
        }
        if rows.is_empty() {
            rows.push(self.row(
                None,
                true,
                "No tweaks installed",
                Some("Add one from GitHub, or put a tweak's folder in the tweaks folder and press Reload.".into()),
                Vec::new(),
            ));
        }
        let reload = center.clone();
        let actions = div()
            .flex()
            .gap_2()
            .when(!self.tweak_install.open, |d| {
                d.child(
                    secondary_button("tweaks-add", "Add from GitHub", false, false)
                        .child(icon("add-01", 14.))
                        .child("Add from GitHub")
                        .on_click(cx.listener(|this, _, _, cx| this.open_tweak_install(cx))),
                )
            })
            .child(tweaks::open_folder_button(root))
            .child(
                secondary_button("tweaks-reload", "Reload", false, false)
                    .child(icon("refresh-cw", 14.))
                    .child("Reload")
                    .on_click(move |_, _, cx| {
                        reload.update(cx, |center, cx| center.reload(cx));
                    }),
            )
            .into_any_element();
        let mut page = Vec::new();
        if self.tweak_install.open {
            page.push(self.tweak_install_group(&center, cx));
        }
        page.push(self.group(
            Some("tweaks-installed"),
            !self.tweak_install.open,
            "Installed",
            Some(
                "Tweaks are off until you turn them on. If a tweak's files change, it turns itself off until you accept the new version."
                    .into(),
            ),
            Some(actions),
            rows,
        ));
        page
    }

    fn open_tweak_install(&mut self, cx: &mut Context<Self>) {
        self.tweak_install.open = true;
        self.tweak_install.stage = Stage::Idle;
        if self.tweak_install.link.is_none() {
            let link = cx.new(|cx| {
                let mut input = Input::field(
                    "GitHub link to a tweak",
                    "owner/repo or a GitHub link to the tweak's folder",
                    content(0.35),
                    cx,
                );
                input.set_font_size(13.);
                input
            });
            self._subscriptions
                .push(cx.subscribe(&link, |_, _, _: &input::Changed, cx| cx.notify()));
            self._subscriptions
                .push(cx.subscribe(&link, |this, _, _: &input::Submitted, cx| {
                    this.look_up_tweak(cx)
                }));
            self.tweak_install.link = Some(link);
        }
        cx.notify();
    }

    /// An install that has started cannot be stopped, so Cancel waits for it.
    pub(in crate::settings) fn cancel_tweak_install(&mut self, cx: &mut Context<Self>) {
        if !matches!(self.tweak_install.stage, Stage::Installing(_)) {
            self.close_tweak_install(cx);
        }
    }

    fn close_tweak_install(&mut self, cx: &mut Context<Self>) {
        self.tweak_install.open = false;
        self.tweak_install.stage = Stage::Idle;
        self.tweak_install.task = None;
        if let Some(link) = &self.tweak_install.link {
            link.update(cx, |input, cx| input.set_text(String::new(), cx));
        }
        cx.notify();
    }

    fn look_up_tweak(&mut self, cx: &mut Context<Self>) {
        if matches!(
            self.tweak_install.stage,
            Stage::Looking | Stage::Installing(_)
        ) {
            return;
        }
        let text = self
            .tweak_install
            .link
            .as_ref()
            .map(|link| link.read(cx).content.to_string())
            .unwrap_or_default();
        let link = match core::parse_link(&text) {
            Ok(link) => link,
            Err(error) => {
                self.tweak_install.stage = Stage::Failed(error);
                cx.notify();
                return;
            }
        };
        self.tweak_install.stage = Stage::Looking;
        let work = cx
            .background_executor()
            .spawn(async move { core::fetch(&link) });
        self.tweak_install.task = Some(cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                this.tweak_install.stage = match result {
                    Ok(fetched) => Stage::Preview(Box::new(fetched)),
                    Err(error) => Stage::Failed(error),
                };
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn confirm_tweak_install(&mut self, cx: &mut Context<Self>) {
        let Some(center) = tweaks::get(cx) else {
            return;
        };
        let stage = std::mem::take(&mut self.tweak_install.stage);
        let Stage::Preview(fetched) = stage else {
            self.tweak_install.stage = stage;
            return;
        };
        let root = center.read(cx).root().to_path_buf();
        let reviewed = fetched.clone();
        self.tweak_install.stage = Stage::Installing(fetched);
        let work = cx
            .background_executor()
            .spawn(async move { core::install(&root, &reviewed) });
        self.tweak_install.task = Some(cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| match result {
                Ok(()) => {
                    center.update(cx, |center, cx| center.reload(cx));
                    this.close_tweak_install(cx);
                }
                Err(error) => {
                    this.tweak_install.stage = Stage::Failed(error);
                    cx.notify();
                }
            });
        }));
        cx.notify();
    }

    #[cfg(test)]
    pub(in crate::settings) fn preview_tweak_for_test(
        &mut self,
        fetched: Fetched,
        cx: &mut Context<Self>,
    ) {
        self.open_tweak_install(cx);
        self.tweak_install.stage = Stage::Preview(Box::new(fetched));
        cx.notify();
    }

    #[cfg(test)]
    pub(in crate::settings) fn confirm_tweak_install_for_test(&mut self, cx: &mut Context<Self>) {
        self.confirm_tweak_install(cx);
    }

    #[cfg(test)]
    pub(in crate::settings) fn tweak_install_open(&self) -> bool {
        self.tweak_install.open
    }

    fn tweak_install_group(
        &mut self,
        center: &Entity<tweaks::Tweaks>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let busy = matches!(
            self.tweak_install.stage,
            Stage::Looking | Stage::Installing(_)
        );
        let installing = matches!(self.tweak_install.stage, Stage::Installing(_));
        let looking = matches!(self.tweak_install.stage, Stage::Looking);
        let has_text = self
            .tweak_install
            .link
            .as_ref()
            .is_some_and(|link| !link.read(cx).content.trim().is_empty());
        let can_look = has_text && !busy;
        let label = if looking { "Looking…" } else { "Look up" };
        let form = div()
            .px_4()
            .py(px(14.))
            .flex()
            .items_center()
            .gap_2()
            .child(
                div()
                    .h(px(32.))
                    .flex_1()
                    .min_w_0()
                    .rounded_lg()
                    .border_1()
                    .border_color(content(0.15))
                    .bg(content(0.03))
                    .px_3()
                    .flex()
                    .items_center()
                    .children(self.tweak_install.link.clone()),
            )
            .child(
                primary_button("tweaks-look-up", label, !can_look)
                    .child(label)
                    .when(can_look, |d| {
                        d.on_click(cx.listener(|this, _, _, cx| this.look_up_tweak(cx)))
                    }),
            )
            .child(
                secondary_button("tweaks-add-cancel", "Cancel", false, installing)
                    .child("Cancel")
                    .when(!installing, |d| {
                        d.on_click(cx.listener(|this, _, _, cx| this.cancel_tweak_install(cx)))
                    }),
            )
            .into_any_element();
        let mut children = vec![form];
        match &self.tweak_install.stage {
            Stage::Failed(error) => children.push(
                div()
                    .debug_selector(|| "settings-tweak-add-error".into())
                    .px_4()
                    .pb(px(14.))
                    .text_size(px(12.))
                    .line_height(px(18.))
                    .text_color(red(0.9))
                    .child(error.clone())
                    .into_any_element(),
            ),
            Stage::Preview(fetched) | Stage::Installing(fetched) => {
                let fetched = fetched.clone();
                children.push(self.tweak_preview(&fetched, center, cx));
            }
            Stage::Idle | Stage::Looking => {}
        }
        self.group(
            Some("tweaks-add"),
            true,
            "Add from GitHub",
            Some("You see exactly what a tweak does before anything is installed.".into()),
            None,
            children,
        )
    }

    /// Everything a tweak would do, read straight from the commit on offer.
    fn tweak_preview(
        &self,
        fetched: &Fetched,
        center: &Entity<tweaks::Tweaks>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let manifest = &fetched.manifest;
        let installing = matches!(self.tweak_install.stage, Stage::Installing(_));
        let note = replace_note(center.read(cx).found(), &manifest.id);
        let label = if installing {
            "Installing…"
        } else {
            "Install"
        };
        div()
            .debug_selector(|| "settings-tweak-preview".into())
            .px_4()
            .py(px(14.))
            .border_t_1()
            .border_color(content(0.05))
            .flex()
            .flex_col()
            .items_start()
            .child(
                div()
                    .text_size(px(13.))
                    .line_height(px(19.5))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(content(1.))
                    .child(title(manifest, Some(&fetched.source))),
            )
            .when(!manifest.description.is_empty(), |d| {
                d.child(
                    div()
                        .mt_1()
                        .text_size(px(12.))
                        .line_height(px(18.))
                        .text_color(content(0.45))
                        .child(manifest.description.clone()),
                )
            })
            .child(div().w_full().child(details(manifest)))
            .when(!prompts(manifest).is_empty(), |d| {
                d.child(prompt_list(
                    "settings-tweak-preview-prompt",
                    prompts(manifest),
                ))
            })
            .child(
                div()
                    .mt(px(14.))
                    .w_full()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_4()
                    .child(
                        div()
                            .min_w_0()
                            .text_size(px(12.))
                            .line_height(px(18.))
                            .text_color(content(0.45))
                            .child(note),
                    )
                    .child(
                        primary_button("tweaks-install", label, installing)
                            .child(label)
                            .when(!installing, |d| {
                                d.on_click(
                                    cx.listener(|this, _, _, cx| this.confirm_tweak_install(cx)),
                                )
                            }),
                    ),
            )
            .into_any_element()
    }

    fn tweak_row(
        &self,
        first: bool,
        tweak: &Tweak,
        center: &Entity<tweaks::Tweaks>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let manifest = &tweak.manifest;
        let on = center.read(cx).is_on(tweak);
        let target = center.clone();
        let switched = tweak.clone();
        let toggle = self.toggle(
            format!("tweak-{}", manifest.id),
            format!("Turn on {}", manifest.name),
            on,
            false,
            cx,
            move |_, enabled, cx| {
                target.update(cx, |center, cx| center.set_enabled(&switched, enabled, cx));
            },
        );
        let description = (!manifest.description.is_empty())
            .then(|| SharedString::from(manifest.description.clone()));
        let id = manifest.id.clone();
        let listed = prompts(manifest);
        let shown = self.tweak_install.shown_prompts.contains(&id);
        let extra = div()
            .flex()
            .flex_col()
            .items_start()
            .when(center.read(cx).is_changed(tweak), |d| {
                d.child(div().mt(px(10.)).child(capability(
                    format!("settings-tweak-changed-{id}"),
                    "alert-circle",
                    cost_color(),
                    "Its files changed. Review it, then turn it on again.".to_owned(),
                )))
            })
            .child(div().w_full().child(details(manifest)))
            .when(!listed.is_empty(), |d| {
                let target = id.clone();
                d.child(
                    div()
                        .id(SharedString::from(format!("tweak-prompts-{id}")))
                        .debug_selector({
                            let id = id.clone();
                            move || format!("settings-tweak-show-prompts-{id}")
                        })
                        .mt(px(8.))
                        .flex()
                        .items_center()
                        .gap_1()
                        .cursor_pointer()
                        .text_size(px(12.))
                        .line_height(px(18.))
                        .text_color(content(0.5))
                        .hover(|style| style.text_color(content(0.8)))
                        .child(
                            icon(
                                if shown {
                                    "chevron-up"
                                } else {
                                    "chevron-down-2"
                                },
                                12.,
                            )
                            .text_color(content(0.4)),
                        )
                        .child(if shown {
                            "Hide prompts"
                        } else {
                            "Show prompts"
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let open = &mut this.tweak_install.shown_prompts;
                            if !open.remove(&target) {
                                open.insert(target.clone());
                            }
                            cx.notify();
                        })),
                )
                .when(shown, |d| {
                    d.child(prompt_list(&format!("settings-tweak-prompt-{id}"), listed))
                })
            })
            .into_any_element();
        self.row_with_details(
            None,
            first,
            title(manifest, tweak.source.as_ref()),
            description,
            Some(extra),
            vec![toggle],
        )
    }
}
