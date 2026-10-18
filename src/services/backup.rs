use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use crate::domain::BackupEntry;

use super::command;

pub fn create(destination: &Path, udid: &str, full: bool) -> Result<String, String> {
    command::ensure_local_path(destination)?;
    fs::create_dir_all(destination)
        .map_err(|error| format!("Could not create backup directory: {error}"))?;
    let destination_str = destination.to_string_lossy().to_string();
    let mut args = vec!["-u", udid, "backup"];
    if full {
        args.push("--full");
    }
    args.push(&destination_str);
    command::checked("idevicebackup2", args)?;
    Ok(format!("Backup completed successfully in {destination_str}"))
}

pub fn restore(source: &Path, udid: &str) -> Result<String, String> {
    command::ensure_local_path(source)?;
    if !source.is_dir() {
        return Err("The selected backup directory does not exist".to_owned());
    }
    let source_str = source.to_string_lossy().to_string();
    match command::checked(
        "idevicebackup2",
        ["-u", udid, "restore", "--skip-apps", &source_str],
    ) {
        Ok(_) => Ok(format!("Restore completed successfully from {source_str}")),
        Err(err) => {
            if err.contains("Find My iPhone must be disabled") || err.contains("ErrorCode 211") {
                Err(
                    "Find My iPhone is enabled on your iPhone (Apple Error Code 211).\n\n\
                    Apple strictly requires Find My iPhone to be temporarily turned OFF before any backup restore can proceed.\n\n\
                    To fix this:\n\
                    1. On your iPhone, open the 'Settings' app.\n\
                    2. Tap your Apple Account / Name at the very top.\n\
                    3. Tap 'Find My' -> 'Find My iPhone'.\n\
                    4. Toggle 'Find My iPhone' to OFF (enter your Apple ID password if prompted).\n\
                    5. Return to iCareFone Clone and click 'Start Transfer' again!\n\n\
                    (Note: You can turn Find My iPhone back ON immediately after the transfer completes!)".to_owned()
                )
            } else {
                Err(err)
            }
        }
    }
}

pub fn disable_encryption(udid: &str, password: &str, target_dir: &Path) -> Result<String, String> {
    command::ensure_local_path(target_dir)?;
    let target_str = target_dir.to_string_lossy().to_string();
    command::checked(
        "idevicebackup2",
        ["-u", udid, "encryption", "off", password, &target_str],
    )
}

pub fn clean_template(root: &Path) -> Result<(), String> {
    if root.exists() {
        fs::remove_dir_all(root).map_err(|e| format!("Could not remove template: {e}"))?;
    }
    Ok(())
}

pub fn list(root: &Path) -> Vec<BackupEntry> {
    let Ok(entries) = fs::read_dir(root) else {
        return Vec::new();
    };
    let mut backups = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        let mut device_name = "iOS Device".to_owned();
        let mut device_udid = path.file_name().unwrap_or_default().to_string_lossy().to_string();
        let mut is_encrypted = false;
        let mut modified = "Unknown".to_owned();

        // Attempt to parse Apple's Info.plist or Manifest.plist
        let info_plist_path = path.join("Info.plist");
        if info_plist_path.is_file() {
            if let Ok(value) = plist::Value::from_file(&info_plist_path) {
                if let Some(dict) = value.as_dictionary() {
                    if let Some(name) = dict.get("Device Name").and_then(|v| v.as_string()) {
                        device_name = name.to_owned();
                    }
                    if let Some(udid) = dict.get("Unique Identifier").and_then(|v| v.as_string()) {
                        device_udid = udid.to_owned();
                    }
                }
            }
        }

        let manifest_plist_path = path.join("Manifest.plist");
        if manifest_plist_path.is_file() {
            if let Ok(value) = plist::Value::from_file(&manifest_plist_path) {
                if let Some(dict) = value.as_dictionary() {
                    if let Some(enc) = dict.get("IsEncrypted").and_then(|v| v.as_boolean()) {
                        is_encrypted = enc;
                    }
                    if let Some(date) = dict.get("Date").and_then(|v| v.as_date()) {
                        modified = date.to_xml_format();
                    }
                }
            }
        }

        if modified == "Unknown" {
            if let Ok(meta) = entry.metadata() {
                if let Ok(time) = meta.modified() {
                    if let Ok(duration) = time.duration_since(UNIX_EPOCH) {
                        modified = format!("Unix {}", duration.as_secs());
                    }
                }
            }
        }

        backups.push(BackupEntry {
            size_bytes: directory_size(&path),
            path,
            device_name,
            device_udid,
            modified,
            is_encrypted,
        });
    }

    backups.sort_by(|left, right| right.modified.cmp(&left.modified));
    backups
}

#[allow(dead_code)]
pub fn inspect_backup_domains(backup_dir: &Path) -> Result<HashMap<String, usize>, String> {
    let manifest_db = backup_dir.join("Manifest.db");
    if !manifest_db.is_file() {
        return Err("Manifest.db was not found in the backup directory".to_owned());
    }
    let connection = rusqlite::Connection::open_with_flags(
        &manifest_db,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .map_err(|err| format!("Failed to open Manifest.db: {err}"))?;

    let mut stmt = connection
        .prepare("SELECT domain, COUNT(*) FROM Files GROUP BY domain")
        .map_err(|err| err.to_string())?;

    let rows = stmt
        .query_map([], |row| {
            let domain: String = row.get(0)?;
            let count: usize = row.get(1)?;
            Ok((domain, count))
        })
        .map_err(|err| err.to_string())?;

    let mut summary = HashMap::new();
    for row in rows.flatten() {
        summary.insert(row.0, row.1);
    }
    Ok(summary)
}

fn directory_size(root: &Path) -> u64 {
    let mut total = 0_u64;
    let mut pending = vec![PathBuf::from(root)];
    while let Some(path) = pending.pop() {
        let Ok(entries) = fs::read_dir(path) else {
            continue;
        };
        for entry in entries.flatten() {
            if let Ok(metadata) = entry.metadata() {
                if metadata.is_dir() {
                    pending.push(entry.path());
                } else {
                    total = total.saturating_add(metadata.len());
                }
            }
        }
    }
    total
}

