use std::fs;
use std::path::{Path, PathBuf};

use crate::domain::AndroidDevice;

use super::command;

const ANDROID_DATABASE_PATHS: &[&str] = &[
    "/sdcard/Android/media/com.whatsapp/WhatsApp/Databases",
    "/sdcard/WhatsApp/Databases",
    "/sdcard/Android/media/com.whatsapp.w4b/WhatsApp Business/Databases",
    "/sdcard/WhatsApp Business/Databases",
];

const ANDROID_MEDIA_PATHS: &[&str] = &[
    "/sdcard/Android/media/com.whatsapp/WhatsApp/Media",
    "/sdcard/WhatsApp/Media",
    "/sdcard/Android/media/com.whatsapp.w4b/WhatsApp Business/Media",
    "/sdcard/WhatsApp Business/Media",
];

pub fn is_adb_available() -> bool {
    command::available("adb")
}

pub fn parse_adb_devices(output: &str) -> Vec<AndroidDevice> {
    let mut devices = Vec::new();

    for line in output.lines().map(str::trim).filter(|l| !l.is_empty()) {
        // Skip daemon start notices and table headers
        if line.starts_with('*') || line.starts_with("List of devices") {
            continue;
        }

        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 2 {
            continue;
        }

        let serial = parts[0].to_owned();
        if serial == "*" || serial.eq_ignore_ascii_case("list") {
            continue;
        }

        let state = parts[1].to_owned();
        let is_ready = state == "device";

        let mut model = String::new();
        for part in &parts[2..] {
            if let Some(m) = part.strip_prefix("model:") {
                model = m.replace('_', " ");
            }
        }

        devices.push(AndroidDevice {
            serial,
            model,
            state,
            is_ready,
        });
    }

    devices
}

pub fn discover_android() -> Result<Vec<AndroidDevice>, String> {
    if !command::available("adb") {
        return Err("ADB is not installed. Run `brew install android-platform-tools` in your terminal to enable Android detection.".to_owned());
    }

    // Run `adb devices -l` to get both serial and model info
    let output = command::checked("adb", ["devices", "-l"])?;
    let mut devices = parse_adb_devices(&output);

    // For any ready devices where model wasn't provided in `adb devices -l`, query getprop
    for dev in &mut devices {
        if dev.is_ready && dev.model.is_empty() {
            let model_prop = command::checked("adb", ["-s", &dev.serial, "shell", "getprop", "ro.product.model"]).ok();
            let brand_prop = command::checked("adb", ["-s", &dev.serial, "shell", "getprop", "ro.product.brand"]).ok();

            let m = model_prop.unwrap_or_default().trim().to_owned();
            let b = brand_prop.unwrap_or_default().trim().to_owned();

            if !m.is_empty() && !b.is_empty() && !m.to_lowercase().starts_with(&b.to_lowercase()) {
                dev.model = format!("{} {}", capitalize(&b), m);
            } else if !m.is_empty() {
                dev.model = m;
            }
        }
    }

    Ok(devices)
}

fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
    }
}

pub fn acquire_public_backup(serial: &str, destination: &Path) -> Result<PathBuf, String> {
    command::ensure_local_path(destination)?;
    fs::create_dir_all(destination).map_err(|error| error.to_string())?;
    let database_root = destination.join("Databases");
    let media_root = destination.join("Media");
    fs::create_dir_all(&database_root).map_err(|error| error.to_string())?;
    fs::create_dir_all(&media_root).map_err(|error| error.to_string())?;

    let mut database_found = false;
    for remote in ANDROID_DATABASE_PATHS {
        let destination_text = database_root.to_string_lossy().into_owned();
        if command::checked("adb", ["-s", serial, "pull", remote, &destination_text]).is_ok() {
            database_found = true;
            break;
        }
    }
    if !database_found {
        return Err("No WhatsApp database backup found on Android. Please open WhatsApp > Settings > Chats > Chat backup, and tap 'Back up' to create a local backup first.".to_owned());
    }

    for remote in ANDROID_MEDIA_PATHS {
        let destination_text = media_root.to_string_lossy().into_owned();
        if command::checked("adb", ["-s", serial, "pull", remote, &destination_text]).is_ok() {
            break;
        }
    }
    Ok(destination.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_adb_devices_with_daemon_notice() {
        let output = r#"
* daemon not running; starting now at tcp:5037
* daemon started successfully
List of devices attached
emulator-5554	device product:sdk_gphone64_arm64 model:Pixel_7 device:emu64a transport_id:1
RFCW10ABCDE	unauthorized usb:337641472X transport_id:2
"#;
        let devices = parse_adb_devices(output);
        assert_eq!(devices.len(), 2);

        assert_eq!(devices[0].serial, "emulator-5554");
        assert_eq!(devices[0].state, "device");
        assert!(devices[0].is_ready);
        assert_eq!(devices[0].model, "Pixel 7");

        assert_eq!(devices[1].serial, "RFCW10ABCDE");
        assert_eq!(devices[1].state, "unauthorized");
        assert!(!devices[1].is_ready);
    }

    #[test]
    fn parses_empty_adb_devices() {
        let output = "List of devices attached\n\n";
        let devices = parse_adb_devices(output);
        assert!(devices.is_empty());
    }
}
