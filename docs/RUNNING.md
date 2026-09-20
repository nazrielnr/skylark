# Panduan Menjalankan Zeron (Running Guide)

Dokumen ini menjelaskan cara membangun (*build*), menjalankan (*run*), mengonfigurasi, dan memecahkan masalah (*troubleshooting*) aplikasi Zeron, khususnya di lingkungan **Windows** dan lingkungan pengembangan lokal.

---

## Daftar Isi
1. [Prasyarat Sistem](#1-prasyarat-sistem)
2. [Menjalankan Zeron (Headed / Mode GUI)](#2-menjalankan-zeron-headed--mode-gui)
3. [Menjalankan Mode Headless (Engine Daemon Saja)](#3-menjalankan-mode-headless-engine-daemon-saja)
4. [Perintah CLI Pendukung](#4-perintah-cli-pendukung)
5. [Konfigurasi & Environment Variables](#5-konfigurasi--environment-variables)
6. [Lokasi Data & File Log](#6-lokasi-data--file-log)
7. [Troubleshooting & Solusi Masalah Umum](#7-troubleshooting--solusi-masalah-umum)

---

## 1. Prasyarat Sistem

### Windows
- **Rust Toolchain**: Stable MSVC (`rustup default stable-x86_64-pc-windows-msvc`).
- **Visual Studio C++ Build Tools & Windows SDK**: Diperlukan untuk Direct3D 11, DXGI, dan ConPTY.
- **Shader Compiler**: Pastikan Windows SDK terpasang. Jika `fxc.exe` tidak terdeteksi otomatis, set environment variable `GPUI_FXC_PATH`.
- **Node.js & npm** (opsional): Diperlukan jika ingin menghubungkan coding agent berbasis JavaScript/TypeScript (seperti Codex, OpenCode, dsb.).
- **Windows 11 Transparency Effects**: Untuk efek kaca *Mica Alt Backdrop* pada UI chrome, aktifkan:
  **Windows Settings > Personalization > Colors > Transparency effects = On**.

---

## 2. Menjalankan Zeron (Headed / Mode GUI)

Zeron adalah aplikasi native dengan subsistem Windows GUI (`#![windows_subsystem = "windows"]`) yang menggunakan framework **GPUI** berbasis DirectX 11.1 / DirectWrite.

### A. Menjalankan Langsung via Cargo (Development)
Cara paling umum saat proses development:

```powershell
cargo run --locked -p zeron
```

Untuk mode build rilis (*release* - performa render maksimal):
```powershell
cargo run --release --locked -p zeron
```

> [!NOTE]
> Pastikan menutup instance Zeron yang sedang berjalan sebelum menjalankan `cargo build` atau `cargo run` kembali, agar Windows tidak mengunci file `zeron.exe` (*file lock error*).

### B. Menjalankan Binary Hasil Kompilasi
Jika binary sudah selesai di-build, Anda dapat menjalankannya langsung tanpa menunggu kompilasi cargo:

**Debug Binary:**
```powershell
.\target\debug\zeron.exe
```

**Release Binary:**
```powershell
.\target\release\zeron.exe
```

Atau cukup **klik dua kali (double click)** pada file `target\debug\zeron.exe` melalui File Explorer.

### C. Menjalankan Secara Detached (Latar Belakang / Mandiri)
Jika Anda meluncurkan Zeron dari PowerShell dan ingin terminal tetap bebas digunakan tanpa menutup jendela Zeron:

```powershell
Start-Process -FilePath ".\target\debug\zeron.exe"
```

---

## 3. Menjalankan Mode Headless (Engine Daemon Saja)

Mode *headless* menjalankan core engine (sync CRDT Loro, adapter agent, IPC server) tanpa merender antarmuka grafis (UI). Sangat ideal untuk server, VPS, atau proses latar belakang.

```powershell
# Menggunakan cargo
cargo run --locked -p zeron -- headless

# Menggunakan binary langsung
.\target\debug\zeron.exe headless
```

---

## 4. Perintah CLI Pendukung

Binary `zeron` menyediakan subcommand CLI untuk administrasi runtime:

```powershell
# Cek status engine, mode sinkronisasi, dan autentikasi
.\target\debug\zeron.exe status

# Introspeksi status sync realtime (koneksi room, ack frames, peers)
.\target\debug\zeron.exe sync

# Login ke akun WorkOS untuk sinkronisasi multi-device
.\target\debug\zeron.exe login

# Keluar dari akun dan kembali ke mode penyimpanan lokal saja
.\target\debug\zeron.exe logout

# Memeriksa pembaruan versi rilis terbaru
.\target\debug\zeron.exe update --check
```

---

## 5. Konfigurasi & Environment Variables

| Variable | Default | Keterangan |
| :--- | :--- | :--- |
| `ZERON_IPC_PORT` | `27654` | Port IPC lokal TCP untuk komunikasi UI ke Engine daemon. |
| `ZERON_DATA_DIR` | `%LOCALAPPDATA%\Zeron` | Direktori penyimpanan database dokumen Loro, sesi chat, dan cache. |
| `RUST_LOG` | `info` (headed/headless), `warn` (CLI) | Filter level logging tracing (contoh: `RUST_LOG=info,zeron_ui=trace`). |
| `GPUI_FXC_PATH` | Otomatis via SDK | Path manual ke compiler HLSL shader `fxc.exe` jika tidak terdeteksi. |
| `CODEX_EXECUTABLE`| Otomatis via PATH | Path override untuk binary agent eksternal jika diperlukan. |

Contoh menjalankan dengan port IPC kustom:
```powershell
$env:ZERON_IPC_PORT = "28999"
cargo run --locked -p zeron
```

---

## 6. Lokasi Data & File Log

Pada Windows, Zeron menyimpan file konfigurasi, database state, dan riwayat log di:

- **Folder Data & Database**:
  `%LOCALAPPDATA%\Zeron\` (atau `C:\Users\<Username>\AppData\Local\Zeron\`)
- **File Log Aplikasi GUI**:
  `%LOCALAPPDATA%\zeron\logs\zeron-headed.log`
- **File Log Engine Headless**:
  `%LOCALAPPDATA%\zeron\logs\zeron-headless.log`

### Cara Memantau Log Secara Real-Time:
Buka jendela PowerShell baru dan jalankan:
```powershell
Get-Content -Wait -Tail 30 "$env:LOCALAPPDATA\zeron\logs\zeron-headed.log"
```

---

## 7. Troubleshooting & Solusi Masalah Umum

### 1. Jendela Tidak Terlihat / Hilang Setelah Dijalankan di Terminal
- **Penyebab**: Jika dijalankan dari subshell script atau runner agen CI/IDE, Windows Job Object secara otomatis membunuh child process saat sesi terminal ditutup.
- **Solusi**: Jalankan aplikasi melalui PowerShell mandiri interaktif, jalankan via File Explorer, atau gunakan:
  ```powershell
  Start-Process -FilePath (Resolve-Path "target\debug\zeron.exe").Path
  ```

### 2. Error: "Access is denied (os error 5)" Saat Build
- **Penyebab**: Proses `zeron.exe` sebelumnya masih aktif berjalan di background sehingga file binary terkunci oleh sistem operasi.
- **Solusi**: Matikan proses zeron yang masih aktif dengan perintah:
  ```powershell
  Stop-Process -Name zeron -Force -ErrorAction SilentlyContinue
  ```
  Kemudian ulangi `cargo build` atau `cargo run`.

### 3. Port IPC Sudah Digunakan (`Address already in use: 27654`)
- **Penyebab**: Ada daemon atau instance Zeron lain yang sedang aktif mendengarkan di port default 27654.
- **Solusi**:
  1. Hentikan instance sebelumnya:
     ```powershell
     Get-Process zeron -ErrorAction SilentlyContinue | Stop-Process -Force
     ```
  2. Atau jalankan instance baru dengan port berbeda menggunakan `ZERON_IPC_PORT`.

### 4. Efek Kaca / Transparansi Jendela Berwarna Abu-Abu Solid
- **Penyebab**: Fitur transparansi Windows 11 dinonaktifkan atau hardware GPU tidak mendukung Mica Alt Backdrop.
- **Solusi**: Buka **Settings Windows > Personalization > Colors** dan pastikan **Transparency effects** dalam posisi **On**.
