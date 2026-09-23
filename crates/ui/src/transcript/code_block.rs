//! Code block highlight caching, copy affordance, and horizontal fit controls.

use std::collections::HashMap;
use std::rc::Rc;
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use gpui::{ClipboardItem, Context, SharedString, Task, Window};
use skylark_syntax::LanguageId as Lang;

use crate::markdown::parser::{Block, BlockTree};
use crate::markdown::render;
use crate::syntax_cache::{DocumentHighlightKey, SyntaxHighlightCache};
use crate::transcript::row::ToolDetail;
use crate::transcript::Transcript;

// ---------------------------------------------------------------------------
// Highlight store (background, time-sliced, paint-only)
// ---------------------------------------------------------------------------

pub(crate) struct HighlightEntry {
    pub(crate) key: DocumentHighlightKey,
    pub(crate) document: Option<Weak<skylark_syntax::HighlightedDocument>>,
    pub(crate) _task: Option<Task<()>>,
}

/// Cache of tokenized code blocks keyed by `(row id, block ix)`. Tokenization
/// runs on the background executor, time-sliced; results apply as paint-only
/// run colors when they land.
#[derive(Default)]
pub(crate) struct HighlightStore {
    pub(crate) entries: HashMap<(SharedString, usize), HighlightEntry>,
    pub(crate) cache: SyntaxHighlightCache,
}

impl HighlightStore {
    /// Current tokens if ready; kicks a background tokenize when stale/missing.
    pub(crate) fn request(
        &mut self,
        row_id: SharedString,
        block_ix: usize,
        lang: Lang,
        code: &str,
        cx: &mut Context<Transcript>,
    ) -> Option<Arc<skylark_syntax::HighlightedDocument>> {
        let slot_key = (row_id.clone(), block_ix);
        let document_key = DocumentHighlightKey::new(lang, code);
        if let Some(entry) = self.entries.get(&slot_key)
            && entry.key == document_key
        {
            let document = entry.document.as_ref()?;
            if let Some(document) = document.upgrade() {
                return Some(document);
            }
        }
        if let Some(document) = self.cache.get(&document_key) {
            self.entries.insert(
                slot_key,
                HighlightEntry {
                    key: document_key,
                    document: Some(Arc::downgrade(&document)),
                    _task: None,
                },
            );
            return Some(document);
        }
        let code = code.to_string();
        let source_bytes = code.len();
        let task = cx.spawn(async move |this, cx| {
            let started = Instant::now();
            let document = cx
                .background_executor()
                .spawn(async move {
                    skylark_syntax::highlight(skylark_syntax::HighlightRequest {
                        source: &code,
                        path: None,
                        fence_tag: Some(match lang {
                            Lang::Rust => "rust",
                            Lang::JavaScript => "javascript",
                            Lang::Jsx => "jsx",
                            Lang::TypeScript => "typescript",
                            Lang::Tsx => "tsx",
                            Lang::Python => "python",
                            Lang::Go => "go",
                            Lang::Json => "json",
                            Lang::Jsonc => "jsonc",
                            Lang::Bash => "bash",
                            Lang::Toml => "toml",
                            Lang::Markdown => "markdown",
                            Lang::Html => "html",
                            Lang::Css => "css",
                            Lang::Yaml => "yaml",
                            Lang::C => "c",
                            Lang::Cpp => "cpp",
                            Lang::CSharp => "csharp",
                            Lang::Java => "java",
                            Lang::Kotlin => "kotlin",
                            Lang::Swift => "swift",
                            Lang::Ruby => "ruby",
                            Lang::Php => "php",
                            Lang::Sql => "sql",
                            Lang::Lua => "lua",
                            Lang::Dockerfile => "dockerfile",
                            Lang::Nix => "nix",
                            Lang::Make => "make",
                        }),
                    })
                    .ok()
                })
                .await;
            this.update(cx, |transcript, cx| {
                if let Some(document) = document {
                    let document = Arc::new(document);
                    let retained = transcript
                        .highlights
                        .cache
                        .insert(document_key, document.clone());
                    if let Some(entry) = transcript.highlights.entries.get_mut(&slot_key)
                        && entry.key == document_key
                    {
                        tracing::debug!(
                            language = ?lang,
                            source_bytes,
                            spans = document.lines.iter().map(Vec::len).sum::<usize>(),
                            elapsed_us = started.elapsed().as_micros() as u64,
                            "syntax highlight ready"
                        );
                        entry.document = retained.then(|| Arc::downgrade(&document));
                        cx.notify();
                    }
                }
            })
            .ok();
        });
        self.entries.insert(
            (row_id, block_ix),
            HighlightEntry {
                key: document_key,
                document: None,
                _task: Some(task),
            },
        );
        None
    }
}

impl Transcript {
    /// Copy-button wiring for one row's code blocks ([`render::CopyUi`]):
    /// click writes the block's code to the clipboard and shows a transient
    /// "Copied" check on that block for ~1.2s (overlay — no layout shift).
    pub(crate) fn copy_ui_for(&self, row_id: &SharedString, cx: &mut Context<Self>) -> render::CopyUi {
        let copied_ix = self
            .copied_code
            .as_ref()
            .filter(|(id, _)| id == row_id)
            .map(|(_, ix)| *ix);
        let row_key = row_id.clone();
        let entity = cx.weak_entity();
        let handler: Rc<dyn Fn(usize, SharedString, &mut Window, &mut gpui::App)> =
            Rc::new(move |ix, code, _window, cx| {
                cx.write_to_clipboard(ClipboardItem::new_string(code.to_string()));
                let row_key = row_key.clone();
                entity
                    .update(cx, |this, cx| {
                        this.copied_code = Some((row_key, ix));
                        this.copied_clear = Some(cx.spawn(async move |this, cx| {
                            cx.background_executor()
                                .timer(Duration::from_millis(1200))
                                .await;
                            this.update(cx, |this, cx| {
                                this.copied_code = None;
                                this.copied_clear = None;
                                cx.notify();
                            })
                            .ok();
                        }));
                        cx.notify();
                    })
                    .ok();
            });
        render::CopyUi { handler, copied_ix }
    }

    /// Interactive layout/scroll plumbing for one agent Markdown fence. The
    /// persisted Fit choice is global, while each block retains only its own
    /// ephemeral horizontal offset and hover/drag state.
    pub(crate) fn code_ui_for(
        &mut self,
        row_id: &SharedString,
        block_ix: usize,
        cx: &mut Context<Self>,
    ) -> render::CodeUi {
        let key: SharedString = format!("{row_id}#code{block_ix}").into();
        let runtime = self.code_fences.entry(key.clone()).or_default();
        render::code_ui_for(
            key,
            crate::settings::current(cx).code_fences_fit_content,
            runtime,
            cx.weak_entity(),
            |transcript| &mut transcript.code_fences,
        )
    }

    pub(crate) fn code_uis_for(
        &mut self,
        row_id: &SharedString,
        block: &Block,
        block_ix: usize,
        cx: &mut Context<Self>,
    ) -> Option<HashMap<usize, render::CodeUi>> {
        let indices = render::code_block_indices(block, block_ix);
        (!indices.is_empty()).then(|| {
            indices
                .into_iter()
                .map(|ix| (ix, self.code_ui_for(row_id, ix, cx)))
                .collect()
        })
    }

    /// Request highlights for the code blocks of a tree. `only` limits to one
    /// block index (split rows); `None` covers the whole tree (live rows).
    pub(crate) fn code_highlight_for(
        &mut self,
        row_id: &SharedString,
        tree: &Arc<BlockTree>,
        only: Option<usize>,
        cx: &mut Context<Self>,
    ) -> HashMap<usize, Option<Arc<skylark_syntax::HighlightedDocument>>> {
        let mut out = HashMap::new();
        for (ix, top) in tree.blocks.iter().enumerate() {
            if only.is_some_and(|o| o != ix) {
                continue;
            }
            if let Block::CodeBlock { language, code } = &top.block
                && let Some(lang) = language
                    .as_deref()
                    .and_then(skylark_syntax::language_for_alias)
            {
                out.insert(
                    ix,
                    self.highlights.request(row_id.clone(), ix, lang, code, cx),
                );
            }
        }
        out
    }

    pub(crate) fn tool_diff_highlight_for(
        &mut self,
        row_id: &SharedString,
        tool_ix: usize,
        detail: &ToolDetail,
        cx: &mut Context<Self>,
    ) -> Option<Arc<crate::changes::DiffHighlights>> {
        let ToolDetail::Diff {
            file,
            old_text,
            new_text,
        } = detail
        else {
            return None;
        };
        let cache_row: SharedString = format!("{row_id}#tool-diff-{tool_ix}").into();
        let old = match old_text {
            Some(source) => {
                let path = file.old_path.as_deref().unwrap_or(&file.path);
                let lang = skylark_syntax::language_for_path(path)?;
                Some(
                    self.highlights
                        .request(cache_row.clone(), 0, lang, source, cx)?,
                )
            }
            None => None,
        };
        let new = match new_text {
            Some(source) => {
                let lang = skylark_syntax::language_for_path(&file.path)?;
                Some(self.highlights.request(cache_row, 1, lang, source, cx)?)
            }
            None => None,
        };
        Some(Arc::new(crate::changes::DiffHighlights { old, new }))
    }
}
