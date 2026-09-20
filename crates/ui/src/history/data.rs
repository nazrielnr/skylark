//! Git history loading, search, view transitions, and pagination.

use super::*;

impl GitHistory {
    pub(super) fn context(&self, cx: &App) -> Option<(String, String, Option<String>)> {
        let state = self.state.read(cx);
        let chat = state.selected_chat_row()?;
        let cwd = chat.cwd.clone()?;
        let target = (state.local_device_id.as_deref() != Some(chat.device_id.as_str()))
            .then(|| chat.device_id.clone());
        let key = format!("{}|{cwd}", target.as_deref().unwrap_or("local"));
        Some((key, cwd, target))
    }

    pub fn ensure_loaded(&mut self, cx: &mut Context<Self>) {
        self.started = true;
        let Some((key, cwd, target)) = self.context(cx) else {
            self.fetch_task = None;
            self.fetching_all = false;
            self.fetch_for = None;
            self.fetch_error = None;
            self.target_key = None;
            self.commits.clear();
            self.visible_commits.clear();
            self.branch_tips.clear();
            self.collapsed_branches.clear();
            self.collapsed_counts.clear();
            self.total_count = None;
            self.head_commit_count = None;
            self.comparison = None;
            self.search_query.clear();
            self.search_results = None;
            self.search_next_cursor = None;
            self.search_total_count = None;
            self.search_loading = false;
            self.search_error = None;
            self.search_generation = self.search_generation.wrapping_add(1);
            self.search_scroll_anchor = None;
            self.search_task = None;
            self.graph = GraphLayout::default();
            self.graph_lane_capacity = 0;
            self.list.reset(0);
            self.hovered_path = None;
            self.hovered_graph_path = None;
            self.hovered_row_path = None;
            self.graph_hover_active = false;
            self.graph_hover_clear_task = None;
            self.avatar_images.clear();
            self.loading = false;
            return;
        };
        if self.target_key.as_deref() == Some(key.as_str()) {
            if !self.loading && self.commits.is_empty() && self.error.is_none() {
                self.fetch_page(key, cwd, target, 0, true, cx);
            }
            return;
        }
        self.request_task = None;
        self.fetch_task = None;
        self.fetching_all = false;
        self.fetch_for = None;
        self.fetch_error = None;
        self.loading = false;
        self.target_key = Some(key.clone());
        self.commits.clear();
        self.visible_commits.clear();
        self.branch_tips.clear();
        self.collapsed_branches.clear();
        self.collapsed_counts.clear();
        self.head_sha = None;
        self.next_cursor = None;
        self.total_count = None;
        self.head_commit_count = None;
        self.comparison = None;
        self.search_query.clear();
        self.search_results = None;
        self.search_next_cursor = None;
        self.search_total_count = None;
        self.search_loading = false;
        self.search_error = None;
        self.search_generation = self.search_generation.wrapping_add(1);
        self.search_scroll_anchor = None;
        self.search_task = None;
        self.error = None;
        self.graph = GraphLayout::default();
        self.graph_lane_capacity = 0;
        self.list.reset(0);
        self.hovered_path = None;
        self.hovered_graph_path = None;
        self.hovered_row_path = None;
        self.graph_hover_active = false;
        self.graph_hover_clear_task = None;
        self.avatar_images.clear();
        self.fetch_page(key, cwd, target, 0, true, cx);
    }

    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let Some((key, cwd, target)) = self.context(cx) else {
            return;
        };
        self.target_key = Some(key.clone());
        self.fetch_page(key, cwd, target, 0, true, cx);
    }

    pub fn fetch_all(&mut self, cx: &mut Context<Self>) {
        if self.fetching_all {
            return;
        }
        let Some((key, cwd, target)) = self.context(cx) else {
            return;
        };
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        self.fetching_all = true;
        self.fetch_for = Some(key.clone());
        self.fetch_error = None;
        cx.notify();
        self.fetch_task = Some(cx.spawn(async move |this, cx| {
            let mut params = serde_json::Map::new();
            params.insert("repoPath".into(), serde_json::Value::String(cwd.clone()));
            if let Some(target) = target.clone() {
                params.insert("targetDeviceId".into(), serde_json::Value::String(target));
            }
            let result = engine
                .client()
                .call(methods::FETCH_ALL, serde_json::Value::Object(params))
                .await;
            this.update(cx, |history, cx| {
                if history.fetch_for.as_deref() != Some(key.as_str()) {
                    return;
                }
                history.fetching_all = false;
                history.fetch_for = None;
                match result {
                    Ok(_) => {
                        history.fetch_error = None;
                        // Cancel a pre-fetch history request so the next page
                        // is guaranteed to observe the updated remote refs.
                        history.request_task = None;
                        history.loading = false;
                        history.fetch_page(key, cwd, target, 0, true, cx);
                        cx.emit(GitHistoryEvent::FetchSucceeded);
                    }
                    Err(error) => history.fetch_error = Some(error.to_string().into()),
                }
                cx.notify();
            })
            .ok();
        }));
    }

    pub fn commit_count(&self) -> Option<usize> {
        if self.search_active() {
            self.search_total_count.or(Some(self.visible_commits.len()))
        } else {
            self.head_commit_count
        }
    }

    pub(super) fn recompute_view(&mut self) {
        if self.search_active() {
            let source = self.search_results.as_ref().unwrap_or(&self.commits);
            let visible: HashSet<_> = source
                .iter()
                .filter(|commit| git_history_matches(&self.search_query, commit))
                .map(|commit| commit.sha.clone())
                .collect();
            self.visible_commits = compact_commits_to_visible(source, &visible);
            self.collapsed_counts.clear();
            self.update_graph_layout();
            return;
        }
        match self.view_mode {
            GitHistoryViewMode::AllCommits => {
                let (visible, counts) = collapse_branch_runs(
                    &self.commits,
                    &self.collapsed_branches,
                    self.head_sha.as_deref(),
                );
                self.visible_commits = visible;
                self.collapsed_counts = counts;
            }
            GitHistoryViewMode::BranchTips => {
                // Immediate parents generally are not tips themselves. Clear
                // them rather than painting false dangling connections; the
                // branch badges carry the stable identity in this overview.
                self.visible_commits = self
                    .branch_tips
                    .iter()
                    .cloned()
                    .map(|mut commit| {
                        commit.parent_shas.clear();
                        commit
                    })
                    .collect();
                self.collapsed_counts.clear();
            }
        }
        self.update_graph_layout();
    }

    pub(super) fn update_graph_layout(&mut self) {
        self.graph = layout_graph(&self.visible_commits, self.head_sha.as_deref());
        // The commit subject starts immediately after the graph column. Keep
        // that column at the widest lane count seen for this repository so a
        // fold cannot move every title sideways when its compact graph settles.
        let loaded_lane_count =
            layout_graph(&self.commits, self.head_sha.as_deref()).max_lane_count;
        self.graph_lane_capacity = self
            .graph_lane_capacity
            .max(self.graph.max_lane_count)
            .max(loaded_lane_count);
    }

    pub(super) fn rebuild_view(&mut self, cx: &mut Context<Self>) {
        self.view_transition = None;
        self.view_transition_task = None;
        self.recompute_view();
        let item_count = self.visible_commits.len() + usize::from(self.has_load_more());
        self.list
            .reset_with_uniform_height(item_count, px(HISTORY_ROW_HEIGHT));
        self.hovered_path = None;
        self.hovered_graph_path = None;
        self.hovered_row_path = None;
        self.graph_hover_active = false;
        self.graph_hover_clear_task = None;
        cx.notify();
    }

    pub(super) fn search_active(&self) -> bool {
        !self.search_query.trim().is_empty()
    }

    pub(super) fn has_load_more(&self) -> bool {
        if self.search_active() {
            self.search_next_cursor.is_some()
        } else {
            self.view_mode == GitHistoryViewMode::AllCommits && self.next_cursor.is_some()
        }
    }

    pub(super) fn current_scroll_anchor(&self) -> Option<HistoryScrollAnchor> {
        let scroll_top = self.list.logical_scroll_top();
        let commit = self
            .visible_commits
            .get(scroll_top.item_ix)
            .or_else(|| self.visible_commits.last())?;
        Some(HistoryScrollAnchor {
            sha: commit.sha.clone(),
            offset_in_item: if scroll_top.item_ix < self.visible_commits.len() {
                scroll_top.offset_in_item
            } else {
                px(0.0)
            },
        })
    }

    pub(super) fn restore_scroll_anchor(&self, anchor: Option<&HistoryScrollAnchor>) {
        let Some(anchor) = anchor else {
            return;
        };
        let Some(item_ix) = self
            .visible_commits
            .iter()
            .position(|commit| commit.sha == anchor.sha)
        else {
            return;
        };
        self.list.scroll_to(ListOffset {
            item_ix,
            offset_in_item: anchor.offset_in_item,
        });
    }

    pub(super) fn reconcile_list_items(
        &self,
        old: &[GitHistoryCommit],
        old_has_load_more: bool,
        target: &[GitHistoryCommit],
        target_has_load_more: bool,
    ) {
        if let Some((range, count)) =
            history_list_splice(old, old_has_load_more, target, target_has_load_more)
        {
            self.list.splice(range, count);
        }
    }

    pub(super) fn settle_view_transition(&mut self, cx: &mut Context<Self>) {
        let Some(transition) = self.view_transition.take() else {
            return;
        };
        // Resolve at settle time rather than reusing the click-time anchor: if
        // the user scrolls during the tween, that newer position must win.
        let final_scroll_anchor = resolve_history_scroll_anchor(
            self.current_scroll_anchor(),
            &self.visible_commits,
            &transition.final_commits,
        );
        let old_has_load_more = self.list.item_count() > self.visible_commits.len();
        let final_has_load_more = self.has_load_more();
        self.reconcile_list_items(
            &self.visible_commits,
            old_has_load_more,
            &transition.final_commits,
            final_has_load_more,
        );
        self.visible_commits = transition.final_commits;
        self.collapsed_counts = transition.final_collapsed_counts;
        self.update_graph_layout();
        self.restore_scroll_anchor(final_scroll_anchor.as_ref());
        self.view_transition_task = None;
        cx.notify();
    }

    pub(super) fn apply_view_change(&mut self, animate_rows: bool, cx: &mut Context<Self>) {
        // A second click during the short tween starts from the previous
        // destination, preventing zero-height transitional rows from leaking
        // into the next merge.
        if self.view_transition.is_some() {
            self.settle_view_transition(cx);
        }
        let scroll_anchor = self.current_scroll_anchor();
        let old_commits = self.visible_commits.clone();
        let old_has_load_more = self.list.item_count() > old_commits.len();
        self.recompute_view();
        let final_commits = std::mem::take(&mut self.visible_commits);
        let final_collapsed_counts = std::mem::take(&mut self.collapsed_counts);
        let final_scroll_anchor =
            resolve_history_scroll_anchor(scroll_anchor.clone(), &old_commits, &final_commits);
        let final_has_load_more = self.has_load_more();

        let unchanged = old_commits.len() == final_commits.len()
            && old_commits
                .iter()
                .zip(&final_commits)
                .all(|(old, new)| old.sha == new.sha);
        if unchanged || !animate_rows || crate::motion::reduced_motion(cx) {
            self.visible_commits = final_commits;
            self.collapsed_counts = final_collapsed_counts;
            self.update_graph_layout();
            self.reconcile_list_items(
                &old_commits,
                old_has_load_more,
                &self.visible_commits,
                final_has_load_more,
            );
            self.restore_scroll_anchor(final_scroll_anchor.as_ref());
            cx.notify();
            return;
        }

        let (transition_commits, rows) = history_transition_rows(&old_commits, &final_commits);
        self.reconcile_list_items(
            &old_commits,
            old_has_load_more,
            &transition_commits,
            final_has_load_more,
        );
        self.visible_commits = transition_commits;
        self.collapsed_counts = final_collapsed_counts.clone();
        self.update_graph_layout();
        self.restore_scroll_anchor(scroll_anchor.as_ref());
        let epoch = self.view_epoch;
        self.view_transition = Some(HistoryViewTransition {
            rows,
            final_commits,
            final_collapsed_counts,
            epoch,
        });
        let duration = crate::motion::COLLAPSE
            .total()
            .mul_f32(crate::motion::speed_scale());
        self.view_transition_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(duration).await;
            this.update(cx, |history, cx| {
                if history
                    .view_transition
                    .as_ref()
                    .is_some_and(|transition| transition.epoch == epoch)
                {
                    history.settle_view_transition(cx);
                }
            })
            .ok();
        }));
        self.hovered_path = None;
        self.hovered_graph_path = None;
        self.hovered_row_path = None;
        self.graph_hover_active = false;
        self.graph_hover_clear_task = None;
        cx.notify();
    }

    pub(super) fn set_view_mode(&mut self, mode: GitHistoryViewMode, cx: &mut Context<Self>) {
        let cleared_individual =
            mode == GitHistoryViewMode::AllCommits && !self.collapsed_branches.is_empty();
        if cleared_individual {
            self.collapsed_branches.clear();
        }
        if self.view_mode == mode && !cleared_individual {
            return;
        }
        self.view_mode = mode;
        self.view_epoch = self.view_epoch.wrapping_add(1);
        self.apply_view_change(false, cx);
    }

    pub(super) fn toggle_branch_ref(&mut self, reference: GitHistoryRef, cx: &mut Context<Self>) {
        let Some(key) = branch_ref_key(&reference) else {
            return;
        };
        self.view_mode = GitHistoryViewMode::AllCommits;
        self.view_epoch = self.view_epoch.wrapping_add(1);
        if !self.collapsed_branches.remove(&key) {
            self.collapsed_branches.insert(key);
        }
        self.apply_view_change(true, cx);
    }

    pub(super) fn set_search_query(&mut self, query: String, cx: &mut Context<Self>) {
        let query = query.trim_start().to_string();
        if self.search_query == query {
            return;
        }
        let was_active = self.search_active();
        if !was_active && !query.trim().is_empty() {
            self.search_scroll_anchor = self.current_scroll_anchor();
        }
        self.search_generation = self.search_generation.wrapping_add(1);
        self.search_task = None;
        self.search_query = query;
        self.search_results = None;
        self.search_next_cursor = None;
        self.search_total_count = None;
        self.search_loading = false;
        self.search_error = None;

        if !self.search_active() {
            self.rebuild_view(cx);
            let anchor = self.search_scroll_anchor.take();
            self.restore_scroll_anchor(anchor.as_ref());
            cx.notify();
            return;
        }

        // The loaded page filters synchronously on the keystroke. The complete
        // repository search replaces it after a tiny debounce without changing
        // topological ordering.
        self.rebuild_view(cx);
        self.list.scroll_to(ListOffset {
            item_ix: 0,
            offset_in_item: px(0.0),
        });
        let query = self.search_query.clone();
        self.request_search_page(query, 0, true, HISTORY_SEARCH_DEBOUNCE, cx);
    }

    pub(super) fn request_search_page(
        &mut self,
        query: String,
        cursor: usize,
        reset: bool,
        delay: Duration,
        cx: &mut Context<Self>,
    ) {
        let Some((key, cwd, target)) = self.context(cx) else {
            return;
        };
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        let generation = self.search_generation;
        self.search_loading = true;
        self.search_error = None;
        cx.notify();
        self.search_task = Some(cx.spawn(async move |this, cx| {
            if !delay.is_zero() {
                cx.background_executor().timer(delay).await;
            }
            let mut params = serde_json::Map::new();
            params.insert("cwd".into(), serde_json::Value::String(cwd.clone()));
            params.insert("query".into(), serde_json::Value::String(query.clone()));
            params.insert("cursor".into(), serde_json::json!(cursor));
            params.insert("limit".into(), serde_json::json!(HISTORY_PAGE_SIZE));
            if let Some(target) = target.clone() {
                params.insert("targetDeviceId".into(), serde_json::Value::String(target));
            }
            let result = engine
                .client()
                .call(
                    methods::SEARCH_GIT_HISTORY,
                    serde_json::Value::Object(params),
                )
                .await;
            this.update(cx, |history, cx| {
                if history.target_key.as_deref() != Some(key.as_str())
                    || history.search_generation != generation
                    || history.search_query != query
                {
                    return;
                }
                history.search_loading = false;
                match result.and_then(|value| {
                    serde_json::from_value::<GitHistoryPage>(value)
                        .map_err(|error| zeron_rpc::RpcError::Failed(error.to_string()))
                }) {
                    Ok(page) => {
                        let anchor = history.current_scroll_anchor();
                        if reset {
                            history.search_results = Some(page.commits);
                        } else {
                            let results = history.search_results.get_or_insert_default();
                            let mut seen: HashSet<_> =
                                results.iter().map(|commit| commit.sha.clone()).collect();
                            results.extend(
                                page.commits
                                    .into_iter()
                                    .filter(|commit| seen.insert(commit.sha.clone())),
                            );
                        }
                        history.search_next_cursor = page.next_cursor;
                        history.search_total_count = page.total_count;
                        history.rebuild_view(cx);
                        history.restore_scroll_anchor(anchor.as_ref());
                        history.resolve_avatars(key, cwd, target, cursor, cx);
                    }
                    Err(error) => {
                        // Keep the instantaneous local matches useful when a
                        // remote/device search is temporarily unavailable.
                        history.search_error = Some(error.to_string().into());
                    }
                }
                cx.notify();
            })
            .ok();
        }));
    }

    pub(super) fn load_older(&mut self, cx: &mut Context<Self>) {
        if self.search_active() {
            if self.search_loading {
                return;
            }
            let Some(cursor) = self.search_next_cursor else {
                return;
            };
            self.request_search_page(self.search_query.clone(), cursor, false, Duration::ZERO, cx);
            return;
        }
        let Some(cursor) = self.next_cursor else {
            return;
        };
        let Some((key, cwd, target)) = self.context(cx) else {
            return;
        };
        self.fetch_page(key, cwd, target, cursor, false, cx);
    }

    pub(super) fn resolve_avatars(
        &mut self,
        key: String,
        cwd: String,
        target: Option<String>,
        cursor: usize,
        cx: &mut Context<Self>,
    ) {
        if configured_author_display(cx) != GitHistoryAuthorDisplay::Avatar {
            return;
        }
        let mut unique_authors = HashMap::new();
        for commit in self
            .commits
            .iter()
            .chain(self.search_results.iter().flatten())
        {
            let email = commit.author_email.trim().to_ascii_lowercase();
            if !email.is_empty() {
                unique_authors
                    .entry(email)
                    .or_insert_with(|| (commit.sha.clone(), commit.author_email.clone()));
            }
        }
        let authors: Vec<_> = unique_authors
            .into_values()
            .map(|(sha, email)| serde_json::json!({ "sha": sha, "email": email }))
            .collect();
        if authors.is_empty() {
            return;
        }
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        let mut params = serde_json::Map::new();
        params.insert("cwd".into(), serde_json::Value::String(cwd));
        params.insert("authors".into(), serde_json::Value::Array(authors));
        params.insert("cursor".into(), serde_json::json!(cursor));
        params.insert("limit".into(), serde_json::json!(HISTORY_PAGE_SIZE));
        if let Some(target) = target {
            params.insert("targetDeviceId".into(), serde_json::Value::String(target));
        }
        cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(
                    methods::RESOLVE_GIT_AVATARS,
                    serde_json::Value::Object(params),
                )
                .await;
            this.update(cx, |history, cx| {
                if history.target_key.as_deref() != Some(key.as_str()) {
                    return;
                }
                if let Ok(avatars) = result.and_then(|value| {
                    serde_json::from_value::<HashMap<String, String>>(value)
                        .map_err(|error| zeron_rpc::RpcError::Failed(error.to_string()))
                }) {
                    history.avatar_images.extend(avatars.into_iter().filter_map(
                        |(email, encoded)| {
                            decode_history_avatar(&encoded).map(|image| (email, image))
                        },
                    ));
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn resolve_loaded_avatars(&mut self, cx: &mut Context<Self>) {
        if self.commits.is_empty() {
            return;
        }
        let Some((key, cwd, target)) = self.context(cx) else {
            return;
        };
        for cursor in (0..self.commits.len()).step_by(HISTORY_PAGE_SIZE) {
            self.resolve_avatars(key.clone(), cwd.clone(), target.clone(), cursor, cx);
        }
    }

    pub(super) fn fetch_page(
        &mut self,
        key: String,
        cwd: String,
        target: Option<String>,
        cursor: usize,
        reset: bool,
        cx: &mut Context<Self>,
    ) {
        if self.loading {
            return;
        }
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        self.loading = true;
        self.error = None;
        cx.notify();
        self.request_task = Some(cx.spawn(async move |this, cx| {
            let mut params = serde_json::Map::new();
            params.insert("cwd".into(), serde_json::Value::String(cwd.clone()));
            params.insert("cursor".into(), serde_json::json!(cursor));
            params.insert("limit".into(), serde_json::json!(HISTORY_PAGE_SIZE));
            if let Some(target) = target.clone() {
                params.insert("targetDeviceId".into(), serde_json::Value::String(target));
            }
            let result = engine
                .client()
                .call(methods::LIST_GIT_HISTORY, serde_json::Value::Object(params))
                .await;
            this.update(cx, |history, cx| {
                if history.target_key.as_deref() != Some(key.as_str()) {
                    return;
                }
                history.loading = false;
                match result.and_then(|value| {
                    serde_json::from_value::<GitHistoryPage>(value)
                        .map_err(|error| zeron_rpc::RpcError::Failed(error.to_string()))
                }) {
                    Ok(page) => {
                        let restart_search = (reset && history.search_active())
                            .then(|| history.search_query.clone());
                        if restart_search.is_some() {
                            history.search_generation = history.search_generation.wrapping_add(1);
                            history.search_task = None;
                            history.search_results = None;
                            history.search_next_cursor = None;
                            history.search_total_count = None;
                            history.search_loading = false;
                            history.search_error = None;
                        }
                        let old_visible_count = history.visible_commits.len();
                        let old_item_count =
                            old_visible_count + usize::from(history.has_load_more());
                        if reset {
                            history.commits = page.commits;
                            history.branch_tips = page.branch_tips;
                            history.total_count = page.total_count;
                            history.head_commit_count = page.head_commit_count;
                            history.comparison = page.comparison;
                        } else {
                            let mut seen: HashSet<String> = history
                                .commits
                                .iter()
                                .map(|commit| commit.sha.clone())
                                .collect();
                            history.commits.extend(
                                page.commits
                                    .into_iter()
                                    .filter(|commit| seen.insert(commit.sha.clone())),
                            );
                            if page.total_count.is_some() {
                                history.total_count = page.total_count;
                            }
                            if page.head_commit_count.is_some() {
                                history.head_commit_count = page.head_commit_count;
                            }
                        }
                        history.head_sha = page.head_sha;
                        history.next_cursor = page.next_cursor;
                        let incremental_all = !history.search_active()
                            && !reset
                            && history.view_mode == GitHistoryViewMode::AllCommits
                            && history.collapsed_branches.is_empty();
                        if incremental_all {
                            history.recompute_view();
                            let new_item_count = history.visible_commits.len()
                                + usize::from(history.next_cursor.is_some());
                            history.list.splice(
                                old_visible_count..old_item_count,
                                new_item_count - old_visible_count,
                            );
                        } else {
                            history.rebuild_view(cx);
                        }
                        history.resolve_avatars(
                            key.clone(),
                            cwd.clone(),
                            target.clone(),
                            cursor,
                            cx,
                        );
                        if let Some(query) = restart_search {
                            history.request_search_page(query, 0, true, Duration::ZERO, cx);
                        }
                    }
                    Err(error) => history.error = Some(error.to_string().into()),
                }
                cx.notify();
            })
            .ok();
        }));
    }

    pub(super) fn copy_sha(&mut self, sha: String, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(sha.clone()));
        self.copied_sha = Some(sha);
        self.copy_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(1_200))
                .await;
            this.update(cx, |history, cx| {
                history.copied_sha = None;
                cx.notify();
            })
            .ok();
        }));
    }
}
