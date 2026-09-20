use std::sync::Arc;

use gpui::Context;
use zeron_rpc::methods;

use super::diff_model::{
    excerpt_highlights, full_highlights, hash64, DiffHighlightState, DiffHighlights, DiffScope,
    FileDiff, HighlightSlot,
};
use super::Changes;

impl Changes {
    /// Start excerpt parsing and a lazy full-source fetch for an expanded file.
    pub(crate) fn request_highlight(
        &mut self,
        file: &FileDiff,
        parsed_key: &str,
        cx: &mut Context<Self>,
    ) -> Option<Arc<DiffHighlights>> {
        let lang = zeron_syntax::language_for_path(&file.path)?;
        let fingerprint = hash64(&[parsed_key, &file.path]);
        if let Some(slot) = self.highlights.get(&file.path)
            && slot.fingerprint == fingerprint
        {
            return match &slot.state {
                DiffHighlightState::Ready(highlights) | DiffHighlightState::Excerpt(highlights) => {
                    Some(highlights.clone())
                }
                DiffHighlightState::Pending | DiffHighlightState::Plain => None,
            };
        }
        if !zeron_syntax::supports_language(lang) {
            self.highlights.insert(
                file.path.clone(),
                HighlightSlot {
                    fingerprint,
                    state: DiffHighlightState::Plain,
                    _excerpt_task: None,
                    _fetch_task: None,
                },
            );
            return None;
        }
        let path = file.path.clone();
        let excerpt_file = file.clone();
        let excerpt_path = path.clone();
        let excerpt_task = cx.spawn(async move |this, cx| {
            let highlights = cx
                .background_executor()
                .spawn(async move { excerpt_highlights(&excerpt_file, lang).map(Arc::new) })
                .await;
            this.update(cx, |changes, cx| {
                if let Some(slot) = changes.highlights.get_mut(&excerpt_path)
                    && slot.fingerprint == fingerprint
                    && matches!(slot.state, DiffHighlightState::Pending)
                {
                    slot.state = match highlights {
                        Some(highlights) => DiffHighlightState::Excerpt(highlights),
                        None => DiffHighlightState::Plain,
                    };
                    cx.notify();
                }
            })
            .ok();
        });

        let active = self.active_diff(cx);
        let engine = self.state.read(cx).engine().cloned();
        let target = self.desired_target(cx);
        let chat_id = self
            .state
            .read(cx)
            .selected_chat_row()
            .map(|chat| chat.id.clone());
        let mode = self.scope.mode().to_string();
        let base_ref = self.base_ref.clone();
        let commit_sha = (self.scope == DiffScope::Commit)
            .then(|| self.commit.as_ref().map(|commit| commit.sha.clone()))
            .flatten();
        let fetch_file = file.clone();
        let fetch_path = path.clone();
        let fetch_task = match (active, engine) {
            (Some(diff), Some(engine)) => Some(cx.spawn(async move |this, cx| {
                let request = zeron_proto::GetCheckoutFileDiffTextRequest {
                    checkout_id: diff.checkout_id,
                    cwd: diff.cwd,
                    path: fetch_path.clone(),
                    mode,
                    base_ref,
                    chat_id,
                    commit_sha,
                    diff_checksum: diff.checksum,
                };
                let mut params = serde_json::to_value(request)
                    .ok()
                    .and_then(|value| value.as_object().cloned())
                    .unwrap_or_default();
                if let Some(target) = target {
                    params.insert("targetDeviceId".into(), serde_json::Value::String(target));
                }
                let response = engine
                    .client()
                    .call(
                        methods::GET_CHECKOUT_FILE_DIFF_TEXT,
                        serde_json::Value::Object(params),
                    )
                    .await
                    .ok()
                    .and_then(|value| {
                        serde_json::from_value::<zeron_proto::CheckoutFileDiffText>(value).ok()
                    });
                let highlights = match response {
                    Some(response) => {
                        cx.background_executor()
                            .spawn(async move {
                                full_highlights(&fetch_file, lang, &response).map(Arc::new)
                            })
                            .await
                    }
                    None => None,
                };
                this.update(cx, |changes, cx| {
                    if let Some(slot) = changes.highlights.get_mut(&fetch_path)
                        && slot.fingerprint == fingerprint
                        && let Some(highlights) = highlights
                    {
                        slot.state = DiffHighlightState::Ready(highlights);
                        cx.notify();
                    }
                })
                .ok();
            })),
            _ => None,
        };
        self.highlights.insert(
            file.path.clone(),
            HighlightSlot {
                fingerprint,
                state: DiffHighlightState::Pending,
                _excerpt_task: Some(excerpt_task),
                _fetch_task: fetch_task,
            },
        );
        None
    }
}
