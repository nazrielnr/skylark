# Windows icon

> **Skylark build status: local-only.** Product login/logout, cloud sync, remote control, updates, public installation, and publishing are temporarily paused. Provider agent authentication remains available. Cloud/mobile/release instructions and results below are historical reference, not current setup guidance. See [current policy](../../docs/LOCAL_ONLY.md).


`skylark.ico` contains 16, 24, 32, 48, 64, 128, and 256 pixel PNG frames
converted from the existing `../macos/icon-1024.png` artwork with bicubic
resampling. It preserves the artwork's transparency.

`apps/skylark/build.rs` compiles `skylark.rc` into the Windows executable for
both debug and release builds. Resource ID 1 is required by GPUI's Windows
icon loader. No installer or adjacent image file is needed at runtime.
