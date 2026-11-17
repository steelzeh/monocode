//! The floating bar for agent changes: its look, and where its arrows go.
//! Agent Changes draws the full bar; every other file tab draws a short one
//! that leads back to the changed files.
use crate::{
    right_panel::{DIFF_ADD, stroke, tabular},
    theme::*,
};
use gpui::{Div, FontWeight, Hsla, Role, Stateful, div, prelude::*, px, rgb};

/// Where the bar floats: centered at the bottom of its pane.
pub(crate) fn overlay(id: &'static str) -> Stateful<Div> {
    div()
        .id(id)
        .role(Role::Toolbar)
        .aria_label("Review changes")
        .absolute()
        .bottom(px(16.))
        .left_0()
        .right_0()
        .flex()
        .flex_wrap()
        .justify_center()
        .gap_2()
}

pub(crate) fn surface() -> Div {
    div()
        .h(px(36.))
        .flex()
        .items_center()
        .gap_1()
        .px_2()
        .rounded_lg()
        .border_1()
        .border_color(stroke())
        .bg(crate::theme::background_base())
        .shadow_lg()
        .text_size(px(12.))
        .font_features(tabular())
        .text_color(content(0.85))
}

/// A text button: Keep is filled green, Undo is outlined.
pub(crate) fn pill(
    id: impl Into<gpui::ElementId>,
    label: &'static str,
    primary: bool,
    enabled: bool,
) -> Stateful<Div> {
    let green: Hsla = rgb(DIFF_ADD).into();
    div()
        .id(id.into())
        .role(Role::Button)
        .aria_label(label)
        .h(px(24.))
        .px(px(9.))
        .flex()
        .flex_shrink_0()
        .items_center()
        .rounded_md()
        .text_size(px(12.))
        .font_weight(FontWeight::MEDIUM)
        .map(|d| {
            if primary {
                d.bg(green.opacity(0.85)).text_color(gpui::black())
            } else {
                d.border_1()
                    .border_color(stroke())
                    .text_color(content(0.85))
            }
        })
        .when(!enabled, |d| d.opacity(0.4))
        .when(enabled, |d| {
            d.cursor_pointer().hover(move |s| {
                if primary {
                    s.bg(green)
                } else {
                    s.bg(content(0.08))
                }
            })
        })
        .child(label)
}

/// Where a tab stands among the changed files, as indexes into them.
#[derive(Debug, PartialEq)]
pub(crate) struct BarStops {
    pub position: Option<usize>,
    pub previous: Option<usize>,
    pub next: Option<usize>,
}

/// From one of the changed files the arrows step along them. From any other
/// tab they lead in: forward to the first file, back to the last.
pub(crate) fn bar_stops(open: Option<&str>, files: &[&str]) -> BarStops {
    let position = open.and_then(|open| files.iter().position(|file| *file == open));
    match position {
        Some(at) => BarStops {
            position,
            previous: at.checked_sub(1),
            next: (at + 1 < files.len()).then_some(at + 1),
        },
        None => BarStops {
            position,
            previous: files.len().checked_sub(1),
            next: (!files.is_empty()).then_some(0),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_arrows_step_along_the_files_or_lead_in_from_another_tab() {
        let files = ["/p/a", "/p/b", "/p/c"];
        let stops = |position, previous, next| BarStops {
            position,
            previous,
            next,
        };
        assert_eq!(
            bar_stops(Some("/p/b"), &files),
            stops(Some(1), Some(0), Some(2))
        );
        assert_eq!(
            bar_stops(Some("/p/a"), &files),
            stops(Some(0), None, Some(1))
        );
        assert_eq!(
            bar_stops(Some("/p/c"), &files),
            stops(Some(2), Some(1), None)
        );
        // The wrong file, or a tab with no single file.
        assert_eq!(
            bar_stops(Some("/p/x"), &files),
            stops(None, Some(2), Some(0))
        );
        assert_eq!(bar_stops(None, &files), stops(None, Some(2), Some(0)));
        assert_eq!(bar_stops(None, &[]), stops(None, None, None));
    }
}
