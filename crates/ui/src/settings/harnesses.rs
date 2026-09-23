//! Settings → Agents: enable/disable harnesses (the t3code models-page
//! arrangement — one card row per agent with a trailing toggle).
//!
//! The state is PER-DEVICE and lives on the engine (`harness-prefs.json` in
//! its data dir): CLI installs are per-device, so enablement is too. The
//! page-header device switcher (the Accounts pattern) retargets both the
//! `ListHarnesses` probe and the `SetHarnessEnabled` writes at any registered
//! device over the relay-forwarded RPCs.
//!
//! Enablement follows DETECTION: every harness whose CLI probe passes is on
//! unless the user switched it off, so installing an agent is all it takes
//! for it to appear here and in the composer. A harness whose CLI is missing
//! on the target device renders dimmed with an install hint and is never
//! enabled (enabling an agent that can't run would only manufacture
//! NotInstalled errors at send time); an ENABLED agent can always be turned
//! OFF except the last one standing — the composer needs something to run.
//! Catalogs from engines predating the detection model can still stamp
//! enabled-but-uninstalled rows; the hint covers that too. The engine
//! enforces the same gates where the state lives, so a raced or stale toggle
//! self-corrects from the RPC reply.

use gpui::{
    AnyElement, Context, Entity, IntoElement, Render, SharedString, Task, Window, div, prelude::*,
    px,
};

use std::time::Duration;
use skylark_engine::registry::TitleSettings;
use skylark_engine::registry::{HarnessDescriptor, descriptor_enabled};

use skylark_proto::Model;
use skylark_proto::{AgentLoginPoll, AgentLoginStart, AgentLoginStatus, HarnessId};
use skylark_rpc::methods;

use crate::pickers::visible_harnesses;
use crate::popover::{self, Loadable};
use crate::settings::widgets;
use crate::state::AppState;
use crate::theme::Theme;

#[path = "harnesses/render.rs"]
mod render;
#[path = "harnesses/titles.rs"]
mod titles;

/// One-line blurb per agent (the t3code models page pairs every toggle row
/// with a description; the catalog descriptor doesn't carry one).
pub fn blurb(harness: HarnessId) -> &'static str {
    match harness {
        HarnessId::ClaudeCode => "Anthropic's coding agent, driven through the Claude Code CLI.",
        HarnessId::Codex => "OpenAI's coding agent, driven through the Codex CLI.",
        HarnessId::Cursor => "Cursor's coding agent, driven through the cursor-agent CLI.",
        HarnessId::Devin => "Cognition's Devin agent (devin CLI).",
        HarnessId::Grok => "xAI's Grok Build agent (grok CLI).",
        HarnessId::Hermes => "Nous Research's Hermes Agent (hermes CLI).",
        HarnessId::Pi => "The pi coding agent (pi CLI).",
        HarnessId::Opencode => "SST's opencode agent (opencode CLI).",
        HarnessId::Antigravity => "Google's Antigravity agent (Antigravity ACP server).",
        HarnessId::Mock => "Scripted test harness.",
    }
}

/// The CLI named in the not-installed hint.
pub fn cli_name(harness: HarnessId) -> &'static str {
    match harness {
        HarnessId::ClaudeCode => "claude",
        HarnessId::Codex => "codex",
        HarnessId::Cursor => "cursor-agent",
        HarnessId::Devin => "devin",
        HarnessId::Grok => "grok",
        HarnessId::Hermes => "hermes",
        HarnessId::Pi => "pi",
        HarnessId::Opencode => "opencode",
        HarnessId::Antigravity => "agy",
        HarnessId::Mock => "mock",
    }
}

pub struct HarnessesPage {
    title_settings: Loadable<TitleSettings>,
    title_models: Loadable<Vec<Model>>,
    title_menu: Option<bool>, // false = harness, true = model
    title_task: Option<Task<()>>,
    title_saving: bool,
    state: Entity<AppState>,
    scroll: widgets::PageScroll,
    harnesses: Loadable<Vec<HarnessDescriptor>>,
    /// Which device's harnesses are shown/edited; `None` = this device (no
    /// passthrough). Retargeted by the page-header device switcher.
    target_device: Option<String>,
    device_menu_open: bool,
    /// Whether the menu was open when the trigger press began — the menu's
    /// `on_mouse_down_out` closes it on that same press, so by click time a
    /// plain toggle would reopen (the [`popover::Popup`] press note, for
    /// this page's bool-state menu).
    device_menu_pressed_open: bool,
    /// Last refused/failed toggle (engine guards), shown in the error strip.
    error: Option<String>,
    load_task: Option<Task<()>>,
    toggle_task: Option<Task<()>>,
    /// a sign-in that switches its harness on once it succeeds.
    sign_in: Option<SignIn>,
    sign_in_failure: Option<SignInFailure>,
    sign_in_task: Option<Task<()>>,
}

struct SignIn {
    harness: HarnessId,
    /// known once the engine accepted the start.
    login_id: Option<String>,
    message: Option<String>,
    phase: SignInPhase,
}

#[derive(Clone, Copy)]
enum SignInPhase {
    Starting,
    Installing,
    Authenticating,
    Enabling,
}

struct SignInFailure {
    harness: HarnessId,
    message: String,
    phase: SignInPhase,
}

impl SignInPhase {
    fn pending_label(self) -> &'static str {
        match self {
            Self::Starting => "Preparing Antigravity…",
            Self::Installing => "Installing Antigravity…",
            Self::Authenticating => "Finish signing in in your browser.",
            Self::Enabling => "Enabling Antigravity…",
        }
    }

    fn failure_label(self) -> &'static str {
        match self {
            Self::Starting => "Setup failed",
            Self::Installing => "Installation failed",
            Self::Authenticating => "Sign-in failed",
            Self::Enabling => "Enable failed",
        }
    }
}

/// harnesses whose toggle runs the agent's own sign-in before switching on.
fn signs_in_on_enable(harness: HarnessId) -> bool {
    harness == HarnessId::Antigravity
}

impl HarnessesPage {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let mut page = Self {
            title_settings: Loadable::Idle,
            title_models: Loadable::Idle,
            title_menu: None,
            title_task: None,
            title_saving: false,
            state,
            scroll: widgets::PageScroll::default(),
            harnesses: Loadable::Idle,
            target_device: None,
            device_menu_open: false,
            device_menu_pressed_open: false,
            error: None,
            load_task: None,
            toggle_task: None,
            sign_in: None,
            sign_in_failure: None,
            sign_in_task: None,
        };
        page.load(cx);
        page
    }

    /// Params with the `targetDeviceId` passthrough merged in.
    fn with_target(&self, mut value: serde_json::Value) -> serde_json::Value {
        if let (Some(target), Some(object)) = (&self.target_device, value.as_object_mut()) {
            object.insert("targetDeviceId".into(), serde_json::json!(target));
        }
        value
    }

    /// Retarget the page at another device: a different device is a different
    /// install/enablement world, so drop the rows and reload through it.
    fn set_target_device(&mut self, target: Option<String>, cx: &mut Context<Self>) {
        self.device_menu_open = false;
        if self.target_device == target {
            cx.notify();
            return;
        }
        self.cancel_sign_in(cx);
        self.title_task = None;
        self.title_settings = Loadable::Idle;
        self.title_models = Loadable::Idle;
        self.title_menu = None;
        self.title_saving = false;
        self.target_device = target;
        self.error = None;
        self.sign_in_failure = None;
        self.harnesses = Loadable::Idle;
        self.load(cx);
        cx.notify();
    }

    /// `ListHarnesses` against the target device (installed probe + enabled
    /// set both come from where the CLIs actually live).
    fn load(&mut self, cx: &mut Context<Self>) {
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        let params = self.with_target(serde_json::json!({}));
        self.load_titles(None, cx);
        self.harnesses = Loadable::Loading;
        self.load_task = Some(cx.spawn(async move |this, cx| {
            let result = engine.client().call(methods::LIST_HARNESSES, params).await;
            this.update(cx, |page, cx| {
                page.harnesses = match result {
                    Ok(value) => match serde_json::from_value::<Vec<HarnessDescriptor>>(value) {
                        Ok(list) => Loadable::Ready(list),
                        Err(err) => Loadable::Error(err.to_string()),
                    },
                    Err(err) => Loadable::Error(err.to_string()),
                };
                cx.notify();
            })
            .ok();
        }));
    }

    fn load_titles(&mut self, save: Option<TitleSettings>, cx: &mut Context<Self>) {
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        let saving = save.is_some();
        let method = if saving {
            methods::SET_TITLE_SETTINGS
        } else {
            methods::GET_TITLE_SETTINGS
        };
        let params = self.with_target(
            save.map(|s| serde_json::to_value(s).unwrap())
                .unwrap_or_else(|| serde_json::json!({})),
        );
        let target = self.target_device.clone();
        self.title_menu = None;
        self.title_saving = saving;
        self.title_task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(method, params)
                .await
                .map_err(|e| e.to_string())
                .and_then(|v| {
                    serde_json::from_value::<TitleSettings>(v).map_err(|e| e.to_string())
                });
            let settings = match result {
                Ok(settings) => settings,
                Err(error) => {
                    this.update(cx, |page, cx| {
                        if saving {
                            page.error = Some(error);
                        } else {
                            page.title_settings = Loadable::Error(error);
                        }
                        page.title_saving = false;
                        cx.notify();
                    })
                    .ok();
                    return;
                }
            };
            let harness = settings.harness;
            this.update(cx, |page, cx| {
                page.title_settings = Loadable::Ready(settings);
                page.title_models = if harness.is_some() {
                    Loadable::Loading
                } else {
                    Loadable::Idle
                };
                page.title_saving = false;
                page.error = None;
                cx.notify();
            })
            .ok();
            if let Some(harness) = harness {
                let result = engine
                    .client()
                    .call(
                        methods::LIST_MODELS,
                        serde_json::json!({"harness": harness, "targetDeviceId": target}),
                    )
                    .await
                    .map_err(|e| e.to_string())
                    .and_then(|v| {
                        serde_json::from_value::<Vec<Model>>(v).map_err(|e| e.to_string())
                    });
                this.update(cx, |page, cx| {
                    page.title_models = match result {
                        Ok(models) => Loadable::Ready(models),
                        Err(error) => Loadable::Error(error),
                    };
                    cx.notify();
                })
                .ok();
            }
        }));
        cx.notify();
    }

    /// Flip one harness on the target device. The reply carries the device's
    /// fresh catalog, so the rows repaint from the authoritative state in one
    /// round trip; refusals (engine guards) land in the error strip.
    fn toggle(&mut self, harness: HarnessId, enabled: bool, cx: &mut Context<Self>) {
        if enabled && signs_in_on_enable(harness) {
            self.start_sign_in(harness, cx);
        } else {
            self.set_enabled(harness, enabled, cx);
        }
    }

    /// sign in first, then switch on: StartAgentLogin, then PollAgentLogin
    /// until the engine reports the outcome, opening the sign-in page the
    /// first time a poll names it.
    fn start_sign_in(&mut self, harness: HarnessId, cx: &mut Context<Self>) {
        if self.target_device.is_some() {
            // the sign-in redirect lands on a loopback port of the device
            // running the agent, which a browser here can't reach
            self.error = Some("Turn this agent on from its own device to sign in.".into());
            cx.notify();
            return;
        }
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        self.error = None;
        self.sign_in_failure = None;
        self.sign_in = Some(SignIn {
            harness,
            login_id: None,
            message: None,
            phase: SignInPhase::Starting,
        });
        let start_params = serde_json::json!({ "harness": harness });
        self.sign_in_task = Some(cx.spawn(async move |this, cx| {
            let started = engine
                .client()
                .call(methods::START_AGENT_LOGIN, start_params)
                .await
                .map_err(|e| e.to_string())
                .and_then(|value| {
                    serde_json::from_value::<AgentLoginStart>(value).map_err(|e| e.to_string())
                });
            let login_id = match started {
                Ok(start) => start.login_id,
                Err(error) => {
                    this.update(cx, |page, cx| {
                        page.fail_sign_in(harness, format!("Sign-in failed to start: {error}"));
                        cx.notify();
                    })
                    .ok();
                    return;
                }
            };
            this.update(cx, |page, _| {
                if let Some(sign_in) = &mut page.sign_in {
                    sign_in.login_id = Some(login_id.clone());
                    sign_in.phase = SignInPhase::Installing;
                }
            })
            .ok();
            let poll_params = serde_json::json!({ "loginId": login_id });
            let mut opened = false;
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(1000))
                    .await;
                let poll = engine
                    .client()
                    .call(methods::POLL_AGENT_LOGIN, poll_params.clone())
                    .await
                    .map_err(|e| e.to_string())
                    .and_then(|value| {
                        serde_json::from_value::<AgentLoginPoll>(value).map_err(|e| e.to_string())
                    });
                let finished = this.update(cx, |page, cx| {
                    let finished = match poll {
                        Ok(poll) => match poll.status {
                            AgentLoginStatus::Pending => {
                                if !opened && let Some(url) = &poll.url {
                                    opened = true;
                                    cx.open_url(url);
                                }
                                if let Some(sign_in) = &mut page.sign_in {
                                    if poll.url.is_some() {
                                        sign_in.phase = SignInPhase::Authenticating;
                                    }
                                    sign_in.message = poll.message;
                                }
                                false
                            }
                            AgentLoginStatus::Done => {
                                page.sign_in_failure = None;
                                if let Some(sign_in) = &mut page.sign_in {
                                    sign_in.phase = SignInPhase::Enabling;
                                    sign_in.message = None;
                                }
                                page.finish_sign_in(harness, cx);
                                true
                            }
                            AgentLoginStatus::Error => {
                                page.fail_sign_in(
                                    harness,
                                    poll.message.unwrap_or_else(|| "Unknown error".into()),
                                );
                                true
                            }
                        },
                        Err(error) => {
                            page.fail_sign_in(harness, error);
                            true
                        }
                    };
                    cx.notify();
                    finished
                });
                if finished.unwrap_or(true) {
                    break;
                }
            }
        }));
        cx.notify();
    }

    fn fail_sign_in(&mut self, harness: HarnessId, message: String) {
        let phase = self
            .sign_in
            .take()
            .filter(|sign_in| sign_in.harness == harness)
            .map(|sign_in| sign_in.phase)
            .unwrap_or(SignInPhase::Starting);
        self.sign_in_failure = Some(SignInFailure {
            harness,
            message,
            phase,
        });
    }

    fn finish_sign_in(&mut self, harness: HarnessId, cx: &mut Context<Self>) {
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            self.fail_sign_in(harness, "Engine unavailable".into());
            return;
        };
        let params = self.with_target(serde_json::json!({
            "harness": harness,
            "enabled": true,
        }));
        self.toggle_task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(methods::SET_HARNESS_ENABLED, params)
                .await
                .map_err(|error| error.to_string())
                .and_then(|value| {
                    serde_json::from_value::<Vec<HarnessDescriptor>>(value)
                        .map_err(|error| error.to_string())
                });
            this.update(cx, |page, cx| {
                match result {
                    Ok(list) => {
                        page.harnesses = Loadable::Ready(list);
                        page.sign_in = None;
                        page.sign_in_failure = None;
                        crate::pickers::bump_harness_catalog(cx);
                    }
                    Err(error) => page.fail_sign_in(harness, error),
                }
                cx.notify();
            })
            .ok();
        }));
    }

    fn cancel_sign_in(&mut self, cx: &mut Context<Self>) {
        let Some(sign_in) = self.sign_in.take() else {
            return;
        };
        self.sign_in_task = None;
        if let (Some(login_id), Some(engine)) =
            (sign_in.login_id, self.state.read(cx).engine().cloned())
        {
            cx.spawn(async move |_, _| {
                if let Err(err) = engine
                    .client()
                    .call(
                        methods::CANCEL_AGENT_LOGIN,
                        serde_json::json!({ "loginId": login_id }),
                    )
                    .await
                {
                    tracing::debug!(error = %err, "CancelAgentLogin failed (best-effort)");
                }
            })
            .detach();
        }
        cx.notify();
    }

    fn set_enabled(&mut self, harness: HarnessId, enabled: bool, cx: &mut Context<Self>) {
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        let params = self.with_target(serde_json::json!({
            "harness": harness,
            "enabled": enabled,
        }));
        self.error = None;
        self.toggle_task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(methods::SET_HARNESS_ENABLED, params)
                .await;
            this.update(cx, |page, cx| {
                match result {
                    Ok(value) => {
                        if let Ok(list) = serde_json::from_value::<Vec<HarnessDescriptor>>(value) {
                            page.harnesses = Loadable::Ready(list);
                        }
                        // The composer caches its catalog per space — poke
                        // every Pickers to re-fetch, or the rail keeps the
                        // old set until restart.
                        crate::pickers::bump_harness_catalog(cx);
                    }
                    Err(err) => page.error = Some(err.to_string()),
                }
                cx.notify();
            })
            .ok();
        }));
    }
}

#[cfg(test)]
mod tests {
    use super::SignInPhase;

    #[test]
    fn antigravity_setup_copy_matches_each_phase() {
        assert_eq!(
            SignInPhase::Starting.pending_label(),
            "Preparing Antigravity…"
        );
        assert_eq!(SignInPhase::Starting.failure_label(), "Setup failed");
        assert_eq!(
            SignInPhase::Installing.pending_label(),
            "Installing Antigravity…"
        );
        assert_eq!(
            SignInPhase::Installing.failure_label(),
            "Installation failed"
        );
        assert_eq!(
            SignInPhase::Authenticating.pending_label(),
            "Finish signing in in your browser."
        );
        assert_eq!(
            SignInPhase::Authenticating.failure_label(),
            "Sign-in failed"
        );
        assert_eq!(
            SignInPhase::Enabling.pending_label(),
            "Enabling Antigravity…"
        );
        assert_eq!(SignInPhase::Enabling.failure_label(), "Enable failed");
    }
}
