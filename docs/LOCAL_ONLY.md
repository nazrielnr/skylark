# Skylark local-only policy

Status: product services temporarily paused. This page takes precedence over historical cloud, mobile, installation, and release instructions elsewhere in this repository.

## Available

Desktop UI, local headless engine, localhost IPC, local sessions and attachments, files, Git, terminals, themes, and local browser previews remain available. Build from the current checkout as described in [RUNNING.md](RUNNING.md).

Provider authentication is separate from Skylark authentication. Claude, Codex, Cursor, and other agent integrations still use their own accounts and services. Adapter installation, Git remotes, and user-opened web pages may also access the network. This policy is not a network sandbox or a guarantee that agents work offline.

## Disabled

| Surface | Current behavior |
| --- | --- |
| Product login and logout | CLI returns an error before touching saved credentials. Account menu does not offer these actions. |
| Cloud/organization RPCs | Sign-in, code exchange, sign-out, organization listing/creation/selection are rejected. Auth status remains observable. |
| Workspace selection | Headed and headless startup always select `WorkspaceScope::Local`, even with saved cloud credentials or cloud environment variables. |
| Cloud transports | No cloud room joins, host relay, peer links, or remote-preview signaling in the desktop runtime. Remote-device RPC forwarding is rejected. |
| Local-to-cloud import | Disabled; no migration or upload. |
| App updater | No background checker, update-status subscription, download, staged apply, or restart-to-update flow. `update --check` also fails without network access. |
| Public installer | `edge/src/install.sh` exits with an explanatory error before downloads or filesystem/service changes. |
| Publishing | Cloud deployment, release, and TestFlight workflows have `.yml.disabled` extensions; GitHub does not execute them. `npm -C edge run deploy` also exits with a disabled error. |
| iOS and websites | Retained source/reference material; not part of the active desktop service offering. |

`SKYLARK_EDGE_URL`, `SKYLARK_EDGE_TOKEN`, `SKYLARK_ORG_ID`, `SKYLARK_WORKOS_CLIENT_ID`, `SKYLARK_WORKOS_API_BASE`, `SKYLARK_CALLBACK_PORT`, `SKYLARK_RELEASES_URL`, and `SKYLARK_AUTO_UPDATE` cannot re-enable these desktop flows. A `skylark-update.json` beside a Windows executable does not enable its updater.

## Data safety

The build does not load, refresh, revoke, delete, migrate, or upload saved product credentials. Existing `session.json` and account stores under `orgs/` remain on disk. They are not automatically shown in the local workspace.

Local data uses `profiles/local/` below the selected application data directory. Zeron's original data directory is not automatically renamed or migrated. Do not remove old stores merely because this local-only build does not display their sessions.

Quit older engines before switching builds. The UI requires a daemon that advertises `skylark-local-only-v1` and a local workspace; it refuses older/cloud-enabled daemons rather than attaching to their cloud-capable runtime.

## Implementation and checks

`crates/proto/src/lib.rs` defines the compile-time `LOCAL_ONLY_BUILD` policy. Engine startup, product RPCs, CLI commands, UI account flows, and update entry points enforce it. It has no runtime environment override.

Transport/auth/update library code remains for isolated tests and future reactivation. Library fixtures and historical benchmark programs are not supported desktop entry points and may deliberately exercise networking.

Focused runtime regression:

```sh
cargo test --locked -p skylark-engine --test local_only_build -j 2
```

Static policy/installer/docs check, without compiling Rust:

```sh
python scripts/check-local-only.py
```

Build/test results recorded in older documents apply to their named revisions, not automatically to this branch.

## Before re-enabling

1. Configure Skylark-owned backend, auth application, release feeds, domains, and signing credentials. Do not restore upstream service defaults.
2. Reconcile Rust/edge wire formats, including preview `skylarkOwned` versus `zeronOwned`, and update cross-language scripts still referring to old crate names.
3. Define explicit account/profile migration and consent. Preserve local and old account stores during migration.
4. Restore product flows and test credentials, transport failures, update integrity, rollback, and platform lifecycle behavior. Reversing the boolean alone does not complete this work.
5. Review deployment ownership before restoring workflow extensions or installer execution. Replace paused documentation with tested instructions.

Dependency and attribution URLs such as `zeronsh/zui` and `zeronsh/gpui-component` remain valid references to upstream source; they are not product service configuration.
