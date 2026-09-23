# Menjalankan Skylark dari source

Skylark sementara hanya mendukung runtime lokal. Login produk, cloud, updater, installer publik, dan publikasi rilis dinonaktifkan. Lihat [kebijakan local-only](LOCAL_ONLY.md). Login provider agent tetap tersedia; mode ini bukan pembatas seluruh akses internet.

## Prasyarat

- Rust sesuai `rust-toolchain.toml` dan Git.
- Windows: toolchain MSVC, Visual Studio C++ Build Tools, Windows SDK, dan CMake. Gunakan `GPUI_FXC_PATH` jika compiler shader `fxc.exe` tidak terdeteksi.
- macOS: Xcode Command Line Tools dan toolchain native.
- Linux: dependency native GPUI serta [runtime browser Linux](reference/linux-browser.md).
- CLI provider yang ingin digunakan; Node.js/npm bila adapter provider memerlukannya. Autentikasi provider tidak memakai login Skylark.

## Menjalankan UI

Dari root checkout:

```sh
cargo run --locked -p skylark
```

Pada Windows, binary hasil build dapat dijalankan dari File Explorer atau PowerShell:

```powershell
.\target\debug\skylark.exe
```

Tutup aplikasi dan engine lama sebelum berpindah build. UI menolak daemon lama yang belum menyatakan dukungan local-only.

## Engine lokal tanpa UI

```sh
cargo run --locked -p skylark -- headless
```

Engine menyediakan IPC pada loopback, bukan kontrol jarak jauh melalui cloud. Hentikan dengan Ctrl+C agar data tersimpan dengan benar. Service lokal launchd/systemd tetap tersedia melalui subcommand `daemon` pada platform yang mendukungnya; installer publik tetap dinonaktifkan.

## Status dan perintah yang dihentikan

```sh
cargo run --locked -p skylark -- status
```

`login`, `logout`, `sync`, `update`, dan `update --check` mengembalikan error fitur dinonaktifkan. Tidak ada petunjuk menggunakan installer upstream atau feed produksi. Untuk mengganti versi selama fase ini, gunakan build dari checkout yang dipilih sendiri.

## Konfigurasi lokal

| Variable | Fungsi |
| --- | --- |
| `SKYLARK_DATA_DIR` | Memilih root data lokal. |
| `SKYLARK_IPC_PORT` | Port IPC loopback; default `27654`. |
| `SKYLARK_HARNESS` | Harness default engine headless, misalnya `mock` untuk pengujian. |
| `SKYLARK_DEVICE_NAME` | Nama perangkat lokal. |
| `RUST_LOG` | Filter log Rust. |
| `GPUI_FXC_PATH` | Lokasi compiler shader Windows jika perlu. |
| `CODEX_EXECUTABLE` | Override executable Codex yang sudah tersedia. |

Variable cloud/auth/update tidak dapat mengaktifkan kembali fitur yang dihentikan. File `skylark-update.json` juga tidak mengaktifkan updater.

## Data dan kredensial

- Windows: `%LOCALAPPDATA%\Skylark`, fallback `%USERPROFILE%\AppData\Local\Skylark`.
- macOS/Linux: `~/.skylark`.
- Data workspace lokal: `profiles/local/` di bawah root tersebut.
- Log: `logs/skylark-headed.log` atau `logs/skylark-headless.log`.

`session.json` dan store akun cloud lama dipertahankan, tetapi tidak dimuat atau diimpor. Riwayat akun cloud tidak otomatis muncul dalam workspace lokal. Folder data Zeron tidak dimigrasikan otomatis. Jangan hapus data lama hanya karena tidak terlihat di build ini.

## Validasi dengan penggunaan disk terbatas

Cek kebijakan tanpa build:

```sh
python scripts/check-local-only.py
```

Pada PowerShell, matikan debug symbols dan incremental untuk pemeriksaan lokal:

```powershell
$env:CARGO_PROFILE_DEV_DEBUG = "0"
$env:CARGO_PROFILE_TEST_DEBUG = "0"
$env:CARGO_INCREMENTAL = "0"
cargo check --locked --workspace -j 2
cargo test --locked -p skylark-engine --test local_only_build -j 2
```

Jangan menjalankan seluruh test workspace sebagai langkah pertama ketika disk terbatas. Rust native UI dan binary test membutuhkan ruang besar.

Untuk menghapus hasil build, tutup aplikasi terlebih dahulu lalu jalankan `cargo clean`. Jika DLL terkunci oleh rust-analyzer, hentikan/restart language server melalui editor sebelum mencoba lagi. Hindari mematikan engine secara paksa saat ada pekerjaan atau file belum tersimpan.

## Gangguan umum

- **Port digunakan:** tutup engine sebelumnya secara normal atau pilih `SKYLARK_IPC_PORT` berbeda. Data dir yang sama tetap hanya boleh dimiliki satu engine.
- **Access is denied saat build:** tutup binary yang sedang berjalan; periksa penguncian DLL oleh editor.
- **Cloud/login tidak tersedia:** perilaku yang disengaja, bukan kesalahan konfigurasi.
- **Agent memerlukan login atau internet:** autentikasi provider terpisah dan tidak dinonaktifkan oleh kebijakan Skylark.

Panduan penelitian, benchmark, iOS, cloud, dan rilis lama tetap ada sebagai referensi. Instruksi yang mengaktifkan cloud atau layanan upstream tidak berlaku pada build ini.
