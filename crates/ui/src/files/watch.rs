//! Document-side handling of workspace watch frames.
//!
//! The tree side (mutations, invalidation, reloads, sequence tracking)
//! lives on the shared [`FileTreeView`](`super::tree_view::FileTreeView`);
//! the shell fans each applied frame out to every Files surface of the
//! panel, and those surfaces reconcile their OPEN DOCUMENTS here.

use gpui::Context;
use skylark_proto::{WorkspaceFileChangeKind, WorkspaceFileChanges};

use super::{FilesEvent, FilesSurface};

impl FilesSurface {
    /// Apply a watch frame the shared tree already applied: reconcile open
    /// documents, images, and rename bookkeeping. `resync` marks a full
    /// refresh (sequence gap or server request) — reconcile everything.
    pub(crate) fn apply_files_frame(
        &mut self,
        frame: &WorkspaceFileChanges,
        resync: bool,
        cx: &mut Context<Self>,
    ) {
        if resync {
            self.invalidate_markdown_images(None, cx);
            self.reconcile_open_documents(cx);
            cx.notify();
            return;
        }

        for change in &frame.changes {
            self.invalidate_markdown_images(Some(&change.path), cx);
            if let Some(old_path) = &change.old_path {
                self.invalidate_markdown_images(Some(old_path), cx);
            }
            match change.kind {
                WorkspaceFileChangeKind::Created => {
                    self.reconcile_created_documents(&change.path, cx);
                }
                WorkspaceFileChangeKind::Modified => {
                    self.reconcile_document(change.path.clone(), cx);
                }
                WorkspaceFileChangeKind::Removed => {
                    self.mark_document_deleted(&change.path, cx);
                }
                WorkspaceFileChangeKind::Renamed => {
                    if let Some(old_path) = &change.old_path {
                        for (old_path, new_path) in
                            self.rename_documents(old_path, change.path.clone(), cx)
                        {
                            cx.emit(FilesEvent::FileRenamed { old_path, new_path });
                        }
                    }
                    // Atomic replacement tools can report a temporary file
                    // being renamed over an already-open destination document.
                    self.reconcile_created_documents(&change.path, cx);
                }
            }
        }
        cx.notify();
    }
}
