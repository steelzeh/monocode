//! Connects tweaks to the app: it shows their panels and carries their app
//! calls. It knows nothing about what any tweak or action does.
use super::*;
use crate::right_panel::RightTab;
use crate::tweaks::{ScriptRun, TweakPanel, TweakPanelEvent};
use monocode_core::{
    mono::bridge::{Call, Reply},
    tweaks::{AppCall, ChatInfo, Context as PanelContext},
};
use serde_json::json;
use std::sync::{Arc, atomic::AtomicBool};

/// Who gets the answer to an app call.
#[derive(Clone)]
enum Caller {
    Script {
        tweak: String,
        generation: u64,
    },
    Panel {
        tweak: String,
        generation: u64,
        view: Entity<TweakPanel>,
    },
}
impl Caller {
    fn tweak(&self) -> &str {
        match self {
            Self::Script { tweak, .. } | Self::Panel { tweak, .. } => tweak,
        }
    }
    fn generation(&self) -> u64 {
        match self {
            Self::Script { generation, .. } | Self::Panel { generation, .. } => *generation,
        }
    }
}

impl Shell {
    pub(super) fn tweak_tab_shown(&self, cx: &App) -> bool {
        crate::tweaks::get(cx).is_some_and(|center| center.read(cx).has_panels())
    }

    fn tweak_context(&self) -> PanelContext {
        let project = &self.workspace.project;
        PanelContext {
            project: std::path::Path::new(project)
                .file_name()
                .map_or(project.clone(), |name| name.to_string_lossy().into_owned()),
        }
    }

    /// Runs on every draw and does nothing unless the tab is on screen and
    /// the user has moved somewhere else or the list of tweaks changed.
    pub(super) fn sync_tweak_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(panel) = self.right_panel.clone() else {
            return;
        };
        let Some(center) = crate::tweaks::get(cx) else {
            return;
        };
        // Before the first read of the tweaks folder nothing is known yet, and
        // a tab restored from the saved layout must not be taken away.
        if center.read(cx).revision() == 0 {
            return;
        }
        let shown = center.read(cx).has_panels();
        panel.update(cx, |panel, cx| panel.set_tweak_tab(shown, cx));
        if panel.read(cx).visible_tab() != Some(RightTab::Tweaks) {
            self.tweak_panel_key = None;
            return;
        }
        let key = (
            self.workspace.project.clone(),
            self.navigation_revision,
            center.read(cx).revision(),
        );
        if self.tweak_panel_key.as_ref() == Some(&key) && panel.read(cx).tweaks().is_some() {
            return;
        }
        self.tweak_panel_key = Some(key);
        let view = match panel.read(cx).tweaks().cloned() {
            Some(view) => view,
            None => {
                let view = cx.new(TweakPanel::new);
                self.observations.push(cx.subscribe_in(
                    &view,
                    window,
                    |this, view, event: &TweakPanelEvent, window, cx| {
                        match event {
                            TweakPanelEvent::Calls(run) => {
                                let caller = Caller::Panel {
                                    tweak: run.tweak.clone(),
                                    generation: run.generation,
                                    view: view.clone(),
                                };
                                let session = this.tweak_panel_session(cx);
                                this.start_tweak_calls(
                                    caller,
                                    session,
                                    run.calls.clone(),
                                    run.cancel.clone(),
                                    window,
                                    cx,
                                );
                            }
                            // The tab in the title bar shows the panel's name.
                            TweakPanelEvent::Selected => this.invalidate_chrome(cx),
                        }
                        cx.notify();
                    },
                ));
                panel.update(cx, |panel, cx| panel.set_tweaks(view.clone(), cx));
                view
            }
        };
        let context = self.tweak_context();
        view.update(cx, |view, cx| view.set_context(context, cx));
    }

    /// The chat a panel's calls run for: the open one, else one in the project.
    fn tweak_panel_session(&self, cx: &App) -> Option<i64> {
        let project = &self.workspace.project;
        let in_project =
            |id: &i64| self.session_metadata(*id, cx).summary.cwd.as_str() == project.as_str();
        self.workspace
            .active
            .map(|pane| self.workspace.session_for_pane(pane))
            .filter(|id| self.session_summaries.contains_key(id))
            .filter(in_project)
            .or_else(|| {
                self.order
                    .iter()
                    .copied()
                    .filter(|id| self.session_summaries.contains_key(id))
                    .find(|id| in_project(id))
            })
    }

    pub(super) fn run_tweak_menu(
        &mut self,
        id: i64,
        action: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some((label, prompt)) = crate::tweaks::menu_prompt(action, cx) {
            let Some(view) = self.ensure_session(id, cx) else {
                return;
            };
            self.activate_session(id, window, cx);
            view.update(cx, |view, cx| {
                view.run_tweak_prompt(label, prompt, false, cx)
            });
            return;
        }
        let Some(script) = crate::tweaks::menu_script(action, cx) else {
            return;
        };
        if self.tweak_working_on(id, &script.tweak) {
            return;
        }
        let chat_id = match locked(&self.store).and_then(|store| store.persisted_session_id(id)) {
            Ok(chat_id) => chat_id,
            Err(error) => {
                self.error = Some(error);
                cx.notify();
                return;
            }
        };
        let meta = self.session_metadata(id, cx);
        let chat = ChatInfo {
            id: chat_id.clone(),
            title: monocode_core::session_title::display_title(
                &meta.summary.title,
                &meta.summary.provider,
            )
            .to_owned(),
            project: std::path::Path::new(&meta.summary.cwd)
                .file_name()
                .map_or(meta.summary.cwd.clone(), |name| {
                    name.to_string_lossy().into_owned()
                }),
        };
        let payload = serde_json::to_string(&chat).unwrap_or_else(|_| "{}".into());
        let result = crate::tweaks::get(cx)
            .ok_or_else(|| "Tweaks are not available".to_owned())
            .and_then(|center| {
                center.update(cx, |center, _| {
                    center.run_menu(&script.tweak, &script.entry, &chat_id, &payload)
                })
            });
        self.after_tweak_script(&script.tweak, id, result, window, cx);
    }

    /// Carries out what a script asked for.
    fn after_tweak_script(
        &mut self,
        tweak: &str,
        session: i64,
        result: Result<ScriptRun, String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match result {
            Ok(run) => {
                let caller = Caller::Script {
                    tweak: tweak.to_owned(),
                    generation: run.generation,
                };
                self.start_tweak_calls(caller, Some(session), run.calls, run.cancel, window, cx);
            }
            Err(error) => self.error = Some(error),
        }
        self.release_tweak_if_idle(tweak, cx);
    }

    fn start_tweak_calls(
        &mut self,
        caller: Caller,
        session: Option<i64>,
        calls: Vec<AppCall>,
        cancel: Arc<AtomicBool>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        for call in calls {
            let tweak = caller.tweak();
            *self.tweak_requests.entry(tweak.to_owned()).or_default() += 1;
            let request = call.request;
            let request_id = format!("tweak:{tweak}:{}:{request}", caller.generation());
            let (reply, answer) = async_channel::bounded(1);
            self.tweak_cancels
                .insert(request_id.clone(), cancel.clone());
            match session {
                Some(session) => self.run_tweak_app_call(
                    Call {
                        session,
                        request_id: request_id.clone(),
                        action: call.action,
                        input: call.input,
                        reply: Reply::App(reply),
                    },
                    window,
                    cx,
                ),
                None => {
                    let _ = reply.try_send(Err("Open a chat in this project first".to_owned()));
                }
            }
            let caller = caller.clone();
            cx.spawn(async move |this, cx| {
                let result = answer
                    .recv()
                    .await
                    .unwrap_or_else(|_| Err("The app did not answer".to_owned()));
                let _ = this.update_in(cx, |this, window, cx| {
                    this.finish_tweak_call(
                        caller,
                        session,
                        request,
                        &request_id,
                        result,
                        window,
                        cx,
                    )
                });
            })
            .detach();
        }
    }

    #[allow(clippy::too_many_arguments)] // Each is one part of a call's identity.
    fn finish_tweak_call(
        &mut self,
        caller: Caller,
        session: Option<i64>,
        request: u64,
        request_id: &str,
        result: Result<serde_json::Value, String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.tweak_cancels.remove(request_id);
        let tweak = caller.tweak().to_owned();
        if let Some(left) = self.tweak_requests.get_mut(&tweak) {
            *left = left.saturating_sub(1);
        }
        let payload = match result {
            Ok(result) => json!({"result": result}),
            Err(error) => json!({"error": error}),
        }
        .to_string();
        match caller {
            Caller::Script { generation, .. } => {
                let result = crate::tweaks::get(cx)
                    .ok_or_else(|| "Tweaks are not available".to_owned())
                    .and_then(|center| {
                        center.update(cx, |center, _| {
                            center.deliver(&tweak, generation, request, &payload)
                        })
                    });
                match session {
                    Some(session) => self.after_tweak_script(&tweak, session, result, window, cx),
                    None => self.release_tweak_if_idle(&tweak, cx),
                }
            }
            Caller::Panel {
                generation, view, ..
            } => {
                view.update(cx, |view, cx| {
                    view.deliver(generation, request, &payload, cx)
                });
                self.release_tweak_if_idle(&tweak, cx);
            }
        }
    }

    /// A tweak with nothing left to wait for cannot leave a chat marked.
    fn release_tweak_if_idle(&mut self, tweak: &str, cx: &mut Context<Self>) {
        if self.tweak_requests.get(tweak).copied().unwrap_or(0) == 0 {
            self.release_working_titles(tweak);
        }
        self.invalidate_chrome(cx);
        cx.notify();
    }
}

#[cfg(test)]
mod tests;
