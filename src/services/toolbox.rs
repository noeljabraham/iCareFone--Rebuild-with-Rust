use std::fs;
use std::path::Path;

use rusqlite::{Connection, OpenFlags};

use crate::domain::Bookmark;

use super::command;

pub fn take_screenshot(udid: &str, destination_file: &Path) -> Result<String, String> {
    command::ensure_local_path(destination_file)?;
    if !command::available("idevicescreenshot") {
        return Err("idevicescreenshot is not installed. Install via `brew install libimobiledevice`".to_owned());
    }
    if let Some(parent) = destination_file.parent() {
        fs::create_dir_all(parent).map_err(|err| err.to_string())?;
    }
    let dest_str = destination_file.to_string_lossy().to_string();
    command::checked("idevicescreenshot", ["-u", udid, &dest_str])?;
    Ok(format!("Screenshot saved to {dest_str}"))
}

pub fn collect_crash_reports(udid: &str, destination_dir: &Path) -> Result<String, String> {
    command::ensure_local_path(destination_dir)?;
    if !command::available("idevicecrashreport") {
        return Err("idevicecrashreport is not installed. Install via `brew install libimobiledevice`".to_owned());
    }
    fs::create_dir_all(destination_dir).map_err(|err| err.to_string())?;
    let dest_str = destination_dir.to_string_lossy().to_string();
    command::checked("idevicecrashreport", ["-u", udid, "-e", &dest_str])?;
    Ok(format!("Crash reports exported to {dest_str}"))
}

pub fn read_bookmarks(db_path: &Path) -> Result<Vec<Bookmark>, String> {
    if !db_path.exists() {
        return Err(format!("Bookmarks database not found: {}", db_path.display()));
    }
    let connection = Connection::open_with_flags(db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|err| format!("Failed to open Bookmarks.db: {err}"))?;

    let mut stmt = connection
        .prepare("SELECT title, url FROM bookmarks WHERE url IS NOT NULL AND url != '' ORDER BY title")
        .map_err(|err| format!("Failed to query bookmarks: {err}"))?;

    let rows = stmt
        .query_map([], |row| {
            Ok(Bookmark {
                title: row.get::<_, Option<String>>(0)?.unwrap_or_else(|| "Untitled".to_owned()),
                url: row.get::<_, String>(1)?,
            })
        })
        .map_err(|err| err.to_string())?;

    let mut bookmarks = Vec::new();
    for row in rows.flatten() {
        bookmarks.push(row);
    }
    Ok(bookmarks)
}

