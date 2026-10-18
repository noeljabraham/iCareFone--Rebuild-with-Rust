# iCareFone Clone

An independent, clean-room, offline macOS utility for iOS device management, backup/restore, photo/media export, contacts extraction, system diagnostics, and WhatsApp Android-to-iOS migration. Written in 100% safe Rust and released under the MIT license.

---

## Reverse Engineering Origin & Clean-Room Architecture

Using **REA** (`rea-agents` reverse-engineering toolkit) to inspect `/Applications/iCareFone.app`, we analyzed:
1. **Binary architecture**: Mach-O 64-bit (`x86_64`) bundle dynamically loading 52 internal frameworks and helper binaries (`ffmpeg`, `tenor_sqlite3`).
2. **Device communication**: The closed-source app internally wrapped MobileDevice and `libimobiledevice` protocols (`TSDeviceCoreManager`, `TSIMobileDevice`, `BackupAndRestore.framework`).
3. **Telemetry & Ads**: Proprietary advertising trackers (`TSAdvertiseTracking.framework`, `BugsplatMac.framework`, Google Analytics logging hooks `TSAsyncGALog`, remote update endpoints on Chinese servers).
4. **Data handling**: Reading and modifying unencrypted iOS backup files (`Manifest.db`, `Info.plist`, SQLite databases).

**iCareFone Clone** implements the same user-facing capabilities cleanly and securely:
- **Zero closed-source trackers or analytics**: No network calls, no analytics, no license checks, completely offline.
- **Direct protocol delegation**: Communicates with iOS devices using the standard open-source `libimobiledevice` CLI tools.
- **Pure Rust performance**: Instant startup, minimal memory footprint, built with `egui` (eframe 0.33) and `rusqlite`.

---

## Features

### 1. Device Dashboard
- Detects connected iPhones and iPads in real time over USB or local WiFi pairing.
- Displays friendly model names (e.g. "iPhone 14 Pro" instead of raw hardware model `iPhone15,2`), iOS version, serial number, and UDID.
- Real-time battery status and disk capacity utilization.
- Quick action navigation shortcuts.

### 2. File & Data Manager
- **Photos & Videos**: Mounts the device camera directory (`DCIM`) using `ifuse` to browse and batch export photos and videos without quality loss.
- **Contacts**: Reads and searches iOS `AddressBook.sqlitedb` databases with complete phone number and email associations; exports directly to standard vCard (`.vcf`) files.
- **Safari Bookmarks**: Parses Safari `Bookmarks.db` SQLite databases and surfaces bookmark titles with clickable URLs.

### 3. Backup & Restore
- Creates full or incremental local iPhone/iPad backups using `idevicebackup2`.
- Inspects saved backup folders, parsing Apple's `Info.plist` and `Manifest.plist` to display device name, backup size, encryption status, and last backup timestamp.
- Safely restores existing backups to a device with confirmation checks.

### 4. WhatsApp Transfer (Android ➔ iOS)
- Pulls local WhatsApp databases from an Android device using ADB.
- Decrypts WhatsApp `Crypt12`, `Crypt14`, and `Crypt15` encrypted backups using AES-256-GCM, HMAC-SHA256, and Zlib decompression.
- Normalizes chats and messages into an intermediate SQLite database.
- Translates and injects messages into an unencrypted iOS `ChatStorage.sqlite` Core Data schema with canonical SHA-1 manifest hashing for restore.

### 5. More Tools & Diagnostics
- One-click device reboot (`idevicediagnostics restart`).
- Device shutdown (`idevicediagnostics shutdown`).
- Device sleep (`idevicediagnostics sleep`).
- Real-time screenshot capture directly to Pictures (`idevicescreenshot`).
- Device crash report extraction (`idevicecrashreport`).

---

## Requirements

- **macOS** (Apple Silicon or Intel, macOS 12+)
- **Rust Toolchain**: 1.85+ (recommended)
- **libimobiledevice**:
  ```sh
  brew install libimobiledevice ifuse
  ```
- **Android Platform Tools** (optional, for Android WhatsApp migration):
  ```sh
  brew install android-platform-tools
  ```

---

## Build & Run

### Running in Development Mode
```sh
cargo run --release
```

### Building the macOS Application Bundle (`.app`)
```sh
./build-app.sh
```
This script compiles the release binary, creates `dist/iCareFone Clone.app` with the proper `Info.plist` bundle metadata, and signs it locally with an ad-hoc certificate. You can then open it directly or drag it into `/Applications`:
```sh
open "dist/iCareFone Clone.app"
```

---

## License

MIT License. See [LICENSE](LICENSE) for details.

