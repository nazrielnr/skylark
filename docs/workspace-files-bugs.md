# Workspace Files: lag & reload investigation

Symptoms as reported (Indonesian, paraphrased):

1. Moving the cursor across the workspace file tree lags — the hover
   highlight "sticks at the start" and trails far behind the pointer.
2. Clicking a file can open the **wrong** file (stale hit-testing while
   frames lag behind the pointer).
3. Every Files tab loads its **own** copy of the workspace: pressing
   Files reloads the tree, state (expansion, selection, scroll) resets,
   and rapid tab opens race directory loads.
4. Switching files quickly via the tree selects the wrong "active" row
   in the tree (the file that opens is correct).
5. Opening a file makes the tree jump from full-pane layout to the right
   sidebar; the loading state flashes at full width with the sidebar
   closed.

## Root causes found

### 1. Hover-style transitions never schedule a draw (Windows)

The gpui fork (`zui`) registers a mouse-move handler for every element
with a hover style; on a transition it calls `cx.notify(current_view)`.
Empirically (sweep test: 841 hover events, 0 resulting re-renders) that
notify path never marks the window dirty on Windows, so the highlight
only appears at the next unrelated draw. Ambient draws run at ~2 Hz
(engine polling), which is the "lag".

Notify from an **element listener** (`cx.listener` → `Context::notify`)
does schedule draws (same sweep: 517 re-renders at ~60 Hz). The tree rows
therefore keep their `.hover()` style for visuals but add:

```rust
.on_hover(cx.listener(|_, _, _, cx| cx.notify()))
```

Platform notes: macOS overrides `frame_requester` (display link);
Wayland runs a self-perpetuating per-vblank frame callback; X11 runs a
refresh-rate timer. Windows has only the vsync `RedrawWindow` thread —
draws happen when something sets dirty, so the broken notify path is
most visible there. The row-listener fix is platform-neutral.

### 2. Per-tab tree copies (race + reload + wrong selection)

Every `FilesSurface` (browser **and** each editor tab) owned its own
`FileTreeModel`, its own load map, and its **own workspace watcher**.
Opening a file cloned the tree into the new tab and re-loaded what the
clone was missing — hence the reload churn, the state resets, and N
concurrent watchers/loads racing for the same directories. Selection
lived per copy, so the *active tab's* tree showed a stale selection
(bug 4).

**Fix — one shared tree per panel.** `FileTreeView`
(`crates/ui/src/files/tree_view.rs`) is now a shell-owned entity keyed by
panel: it holds the model, the single-flight directory loads, the one
watcher, and the scroll/focus state. Every Files surface embeds the same
entity, so:

* new tabs never reload the workspace (root loads exactly once),
* expansion/selection/scroll persist across tabs,
* rapid clicks cannot race loads (one in-flight map),
* the visible tree always shows the row that was clicked.

Watch frames mutate the tree once and emit
`WorkspaceTreeEvent::DocumentsChanged`; the shell fans the frame out to
the panel's surfaces, which reconcile their open documents (the doc-side
of the old watcher lives in `files/watch.rs`).

### 3. Browser → editor layout snap

The raw workspace browser renders the tree at full pane width; an editor
tab renders it as a right sidebar. Switching tabs snapped between the
two layouts (bug 5), and a fresh editor tab measured `surface_width`
from a stale initial value, so the "Loading file…" state rendered at
full width until the file arrived.

**Fix — width tweens in both layout branches + a shared resting width.**

The sidebar's resting width lives on the shared `FileTreeView`
(`sidebar_width`), so a drag on one tab persists across every tab of
the panel — per-surface copies used to make each tab rest at a
different width (the "jarak resize berbeda" report).

Opening a file seeds the new tab with the width the tree already has
(full pane from the browser, the shared sidebar width from a file
tab); `FilePreviewState::seed_sidebar_transition` starts a one-shot
ease toward the resting width (200 ms spec, cancel on drag). The first
frame matches the previous layout exactly, the sidebar then slides
right, and the file preview grows into the freed space. Activation
snaps openness (only the explicit toggle animates), so the loading
state never renders across the tree region — the probes report 0 px
preview/tree overlap on every frame.

Returning to the raw Files tab runs the reverse: `begin_raw_expand`
eases the tree from the sidebar width back to the full pane width,
anchored right and growing leftward.

Seeding `surface_width` also fixes the wide/narrow branch on frame 1
(the measuring canvas only runs after the first paint).

## Verification

`tools/ui_debug/run_scenario.ps1 -Scenario smoke` drives the real app
and asserts: hover repaint activity, 1:1 click→select→open→tab, last
selection == last open, zero root reloads during tab opens, one browser
surface. The tree-geometry trace shows the sidebar easing
`L=681 → 1087` over ~8 frames after the first file click.

See `docs/ui-tracing.md` for the trace vocabulary and how to add
scenarios.
