# Skylark

Local desktop controller for coding agents, based on [Zeron](https://github.com/zeronsh/zeron).

*English | [简体中文](README.zh-CN.md)*

## Current status: local-only development

Product login/logout, cloud sync, organization setup, remote-device control, local-to-cloud import, and application updates are temporarily disabled. Saved cloud sessions and account data are preserved but not loaded. Cloud environment variables and portable update-feed configuration cannot enable these features.

Local chat history, files, Git, terminals, and local previews remain available. Provider agent authentication and adapter installation remain enabled: agents, Git remotes, package downloads, and pages you open can still use the network. **Local-only does not mean an offline sandbox.**

## Run from source

Use your current checkout; public installers, downloads, and release publishing are paused. Do not use upstream installers to install Skylark.

With Rust and your platform's build tools installed:

```sh
cargo run --locked -p skylark
```

For a local engine without the UI:

```sh
cargo run --locked -p skylark -- headless
```

See [the running guide](docs/RUNNING.md), [Windows prerequisites](docs/reference/windows-development.md), and [Linux browser requirements](docs/reference/linux-browser.md).

`skylark status` reports local engine status. `login`, `logout`, `sync`, and `update` return a disabled error; they do not contact product services or modify saved cloud credentials. Quit older Zeron/Skylark engines before starting this build.

## Development boundaries

- Runtime behavior and re-enablement requirements: [Local-only policy](docs/LOCAL_ONLY.md).
- Native UI dependency repositories remain upstream dependencies, pinned in `Cargo.toml` and `Cargo.lock`.
- `edge/`, `apps/ios/`, and website sources are retained for future work, not active Skylark services.
- Cloud, release, and TestFlight workflows have `.yml.disabled` extensions. Source build/test workflows remain active.
- Historical cloud architecture and test reports remain reference material, not current setup instructions.

Skylark does not migrate Zeron's data directory automatically. See the running guide before selecting a data directory.

## License and attribution

Based on Zeron under the [MIT License](LICENSE). Original copyright notices and [third-party notices](THIRD_PARTY_NOTICES.md) are retained. Renaming the app does not rename or transfer ownership of upstream services.
