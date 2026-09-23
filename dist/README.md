# Desktop packaging status

Public installation and release publishing are temporarily paused. Build and run from your checkout using [the running guide](../docs/RUNNING.md). See [the local-only policy](../docs/LOCAL_ONLY.md) for disabled product services and data handling.

## Retained packaging assets

- `scripts/package-linux.sh`: local Linux packaging and desktop integration assets.
- `scripts/package-macos.sh`: native macOS bundle and disk-image tooling.
- `scripts/package-windows.ps1`: native Windows portable packaging.
- `dist/macos/` and `dist/windows/`: platform resources and icons.

These remain development tools, not a supported public installation or update channel. Generated manifests, update payloads, or a Windows `skylark-update.json` do not enable the app's disabled updater. The public `edge/src/install.sh` exits before downloading or installing anything.

The release workflow is retained as `.github/workflows/release.yml.disabled` and does not run on tags or manual dispatch. Cloud deployment and TestFlight publishing are also disabled. Do not upload these development builds to upstream infrastructure.

## Distribution requirements retained

Keep `LICENSE`, `THIRD_PARTY_NOTICES.md`, and bundled third-party license files with any distributed build. Asset filename changes do not replace the original artwork. Signing, notarization, Skylark-owned release hosting, final branding, and updater compatibility still require review before public distribution resumes.
