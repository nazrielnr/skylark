# Project selector device names

> **Skylark build status: local-only.** Product login/logout, cloud sync, remote control, updates, public installation, and publishing are temporarily paused. Provider agent authentication remains available. Cloud/mobile/release instructions and results below are historical reference, not current setup guidance. See [current policy](../../LOCAL_ONLY.md).


- `before.png`: user-supplied macOS screenshot showing missing host labels.
- `after.png`: native Linux/X11 capture of the production Shell with synthetic data, showing duplicate project names, an offline host, long labels, and the unknown-device fallback.
- `filtered.png`: the same native fixture after searching for `anara`.

Reproduce with `cargo run -p skylark-ui --example project-selector-fixture` on a desktop (or Xvfb with a window manager). Open **All projects**, then type `anara`. Down and Return select the second matching project; the trigger shows `anara @ Build server` and its offline glyph.

The fixture uses temporary settings and in-memory data without starting an engine or connecting to a real device. These captures validate rendering and picker interaction, not device synchronization. The after captures use Linux window rendering; they do not validate the macOS compositor.

Validation: `cargo build -p skylark`; `cargo build -p skylark-ui --example project-selector-fixture`; `cargo test -p skylark-ui --lib state::tests:: -- --test-threads=1` (54 passed); rustfmt checks on the changed Rust files; `git diff --check`.
