# Workspace Files UI tracing

`ZERON_UI_TRACE=1` (env) turns on `[trace]` lines on **stderr**. They are
no-ops otherwise: one atomic load per check, nothing else in shipping
builds. `crate::ui_trace::init()` runs in `run_app`; the `ui_trace!` macro
and `ui_trace::bounds_probe` are the only hooks.

## Vocabulary

| Line | Meaning | Emitted from |
| --- | --- | --- |
| `tree-load dir=? cursor=?` | a directory page load starts | `FileTreeView::load_directory` |
| `tree-select path=?` | a row was activated (selection set) | `FileTreeView::activate_path` |
| `tree-open path=?` | a file row asked the shell for a tab | same, file branch |
| `tree-rows count L T R B content_h paths=…` | one render's geometry; `paths` is up to 24 `kind:path` entries (`d` dir, `f` file) | `FileTreeView::render` |
| `surface-new kind=files panel=?` | the per-panel browser surface was created | `Shell::add_files_surface` |
| `surface-new kind=file id path=?` | an editor tab was created | `Shell::add_file_surface` |
| `watch-frame seq changes resync` | the single watcher applied a frame | `FileTreeView::apply_tree_changes` |
| `bounds <id> L T R B` | a probe element's on-screen geometry | `ui_trace::bounds_probe` |

Geometry values are **window-local logical pixels**. Convert to physical
screen coordinates with `client origin + value × (DPI / 96)`.

## Driver

`tools/ui_debug/run_scenario.ps1` (Windows) launches the app with tracing,
opens the right pane (`Ctrl+R`), opens Files (clicks the picker card),
then runs a scenario and asserts against the log:

```
powershell -ExecutionPolicy Bypass -File tools/ui_debug/run_scenario.ps1
# -Scenario hover   : hover-sweep only
# -Scenario tabs    : rapid file-row clicks only
# -Scenario smoke   : both (default)
```

The `tabs` scenario asserts the workspace-files invariants:

* every row click selects exactly once,
* each file click opens/activates exactly one tab,
* the LAST selection equals the LAST open (the visible tree shows the
  file that was actually clicked),
* the workspace **root never reloads** while tabs open,
* only one browser Files surface exists per panel,
* clicking re-reads the geometry after every click (the tree moves when
  the layout changes — browser full pane → editor sidebar).

Notes:

* The window needs focus; the script drives the real cursor — do not
  touch the mouse while it runs.
* Boot can be slow (engine connect); the script waits and retries
  `Ctrl+R`.
* Row geometry comes from `content_h / count` (uniform rows), not from
  the viewport height.

## Adding a scenario

Parse with `Select-String`/regex like the existing ones, keep assertions
in the `$failures` array, and end with the `---- RESULT ----` block so the
exit code stays meaningful for CI-style use.
