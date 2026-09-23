# Appshots UX refinement implementation plan

> **Skylark build status: local-only.** Product login/logout, cloud sync, remote control, updates, public installation, and publishing are temporarily paused. Provider agent authentication remains available. Cloud/mobile/release instructions and results below are historical reference, not current setup guidance. See [current policy](../LOCAL_ONLY.md).


> Historical design notes. The implemented behavior and current validation are
> documented in [Appshots](../appshots.md); proposals below may be superseded.

1. Extend `crates/ui/src/appshots.rs` and `appshots/macos.rs` with optional
   application-icon presentation data and separate Screen Recording and
   Accessibility permission requests.
2. Replace the vertical Appshot rows in `crates/ui/src/composer.rs` with a
   fixed-height horizontal visual tray modeled on the proven Codex treatment;
   preserve preview, removal, upload, routing, and prompt serialization.
3. Rework `crates/ui/src/settings/shortcuts.rs` into an explicit permission
   checklist. Do not request permissions merely because the feature toggle was
   enabled, and label Accessibility as optional.
4. Update pure layout tests and permission-facing copy. Validate with
   `cargo test -p skylark-ui --lib`, `cargo check -p skylark-ui`,
   `cargo build -p skylark`, and `git diff --check`.

## Composer visual normalization follow-up

1. Record the PNG pixel dimensions in `crates/ui/src/appshots.rs` and
   `appshots/macos.rs`, then compute an explicit contained display size for a
   shared 208×132 image box in `crates/ui/src/composer.rs`.
2. Make both the visual stage and the complete Appshot tile clipping
   boundaries, bottom-align every aspect ratio, and apply a native bottom
   edge-fade to the screenshot before layering the application icon.
3. Increase the icon-to-label spacing while keeping one fixed-height,
   horizontally scrolling tray. Capture, removal, preview, persistence,
   transport, and permission controls remain unchanged.
4. Add dimension-parser and landscape/portrait/square sizing tests, run the
   `skylark-ui` test suite and macOS build checks, then inspect the result in the
   signed `Skylark Dev.app`.
