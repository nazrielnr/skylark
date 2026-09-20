//! Overlay, dialog, and splash screen presentation logic for the shell.

use gpui::{AnyElement, Context, IntoElement, Pixels, Window};

use crate::loaders;
use crate::theme::Theme;

use super::Shell;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SplashPhase {
    Visible,
    FadingOut,
    Gone,
}

impl Shell {
    pub(super) fn render_overlays(
        &mut self,
        viewport: gpui::Size<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let mut overlays: Vec<AnyElement> = Vec::new();

        if let Some(overlay) = self.render_chat_menu_overlay(cx) {
            overlays.push(overlay);
        }

        if let Some(overlay) = self.render_rename_dialog_overlay(viewport, window, cx) {
            overlays.push(overlay);
        }

        overlays.extend(self.render_space_overlays(viewport, window, cx));
        if let Some(overlay) = self.render_command_palette(viewport, window, cx) {
            overlays.push(overlay);
        }
        if let Some(overlay) = self.render_add_space_overlay(viewport, window, cx) {
            overlays.push(overlay);
        }
        if let Some(overlay) = self.render_project_action_overlay(viewport, window, cx) {
            overlays.push(overlay);
        }

        if let Some(overlay) = self.render_delete_confirm_dialog_overlay(viewport, cx) {
            overlays.push(overlay);
        }

        if let Some(sync) = self.render_sync_overlay(viewport, cx) {
            overlays.push(sync);
        }

        overlays
    }

    pub(super) fn render_splash(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let theme = Theme::of(cx).clone();
        match self.splash {
            SplashPhase::Visible => {
                Some(loaders::splash_overlay(&theme, false, cx.entity_id(), cx).into_any_element())
            }
            SplashPhase::FadingOut => {
                Some(loaders::splash_overlay(&theme, true, cx.entity_id(), cx).into_any_element())
            }
            SplashPhase::Gone => None,
        }
    }
}
