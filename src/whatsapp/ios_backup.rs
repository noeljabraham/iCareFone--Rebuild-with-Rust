use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use rusqlite::types::Value;
use rusqlite::{Connection, OptionalExtension, params, params_from_iter};
use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};
use walkdir::WalkDir;

const IOS_EPOCH_OFFSET: f64 = 978_307_200.0;
const WHATSAPP_DOMAINS: &[&str] = &[
    "AppDomainGroup-group.net.whatsapp.WhatsApp.shared",
    "AppDomain-net.whatsapp.WhatsApp",
];

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct IosTemplate {
    pub backup_root: PathBuf,
    pub file_id: String,
    pub domain: String,
    pub relative_path: String,
    pub chat_storage: PathBuf,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PrepareResult {
    pub output_backup: PathBuf,
    pub conversations_written: u64,
    pub messages_written: u64,
    pub warnings: Vec<String>,
}

pub fn is_valid_unencrypted_backup(root: &Path) -> bool {
    if !root.is_dir() {
        return false;
    }
    let manifest = root.join("Manifest.db");
    if !manifest.is_file() {
        return false;
    }
    let manifest_plist = root.join("Manifest.plist");
    if manifest_plist.is_file() {
        if let Ok(value) = plist::Value::from_file(&manifest_plist) {
            if let Some(dict) = value.as_dictionary() {
                if let Some(true) = dict.get("IsEncrypted").and_then(|v| v.as_boolean()) {
                    return false;
                }
            }
        }
    }
    if let Ok(file) = fs::File::open(&manifest) {
        use std::io::Read;
        let mut header = [0u8; 16];
        if (&file).take(16).read_exact(&mut header).is_ok() {
            return &header == b"SQLite format 3\0";
        }
    }
    false
}

pub fn inspect_template(root: &Path) -> Result<IosTemplate, String> {
    if !root.is_absolute() || !root.is_dir() {
        return Err("The iOS template backup must be an existing absolute directory".to_owned());
    }
    let manifest = root.join("Manifest.db");
    if !manifest.is_file() {
        return Err("Manifest.db was not found in the iOS backup".to_owned());
    }

    // Verify Manifest.plist encryption flag
    let manifest_plist = root.join("Manifest.plist");
    if manifest_plist.is_file() {
        if let Ok(value) = plist::Value::from_file(&manifest_plist) {
            if let Some(dict) = value.as_dictionary() {
                if let Some(true) = dict.get("IsEncrypted").and_then(|v| v.as_boolean()) {
                    return Err(
                        "The iOS backup template is encrypted ('Encrypt local backup' is enabled on your iPhone).\n\
                        Apple encrypts Manifest.db and ChatStorage.sqlite with AES-256, which prevents offline WhatsApp database injection.\n\n\
                        To fix this:\n\
                        1. Open macOS Finder and select your iPhone in the left sidebar.\n\
                        2. Under the 'General' tab in the 'Backups' section, UNCHECK 'Encrypt local backup'.\n\
                        3. Enter your iPhone backup password when prompted by macOS.\n\
                        4. Return to iCareFone Clone and click 'Start Transfer' again!".to_owned()
                    );
                }
            }
        }
    }

    // Check Manifest.db header before SQLite attempts to parse it
    let manifest_bytes = fs::read(&manifest).map_err(|error| error.to_string())?;
    if !manifest_bytes.starts_with(b"SQLite format 3\0") {
        return Err(
            "Manifest.db is not an unencrypted SQLite database (it is encrypted with your iPhone backup password or corrupt).\n\
            To fix this, uncheck 'Encrypt local backup' in macOS Finder, then retry the transfer.".to_owned()
        );
    }

    let connection =
        Connection::open_with_flags(&manifest, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|error| format!("Could not open Manifest.db: {error}"))?;
    let mut match_row = None;
    for domain in WHATSAPP_DOMAINS {
        match_row = connection
            .query_row(
                "SELECT fileID,domain,relativePath FROM Files
                 WHERE domain=?1 AND (relativePath='ChatStorage.sqlite' OR relativePath LIKE '%/ChatStorage.sqlite')
                 ORDER BY length(relativePath) ASC LIMIT 1",
                [domain],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .optional()
            .map_err(|error| error.to_string())?;
        if match_row.is_some() {
            break;
        }
    }
    if match_row.is_none() {
        match_row = connection
            .query_row(
                "SELECT fileID,domain,relativePath FROM Files
                 WHERE domain LIKE '%whatsapp%' AND (relativePath='ChatStorage.sqlite' OR relativePath LIKE '%/ChatStorage.sqlite')
                 ORDER BY length(relativePath) ASC LIMIT 1",
                [],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .optional()
            .map_err(|error| error.to_string())?;
    }
    let (file_id, domain, relative_path) = match_row.ok_or_else(|| {
        "The backup does not contain WhatsApp ChatStorage.sqlite. Install and initialize WhatsApp on the iPhone before creating the template backup.".to_owned()
    })?;
    let expected_file_id = backup_file_id(&domain, &relative_path);
    if file_id != expected_file_id {
        return Err("Manifest.db contains a noncanonical WhatsApp file identifier".to_owned());
    }
    let chat_storage = root.join(&file_id[0..2]).join(&file_id);
    if !chat_storage.is_file() {
        return Err("Manifest.db references a missing ChatStorage.sqlite backup file".to_owned());
    }
    let header = fs::read(&chat_storage).map_err(|error| error.to_string())?;
    if !header.starts_with(b"SQLite format 3\0") {
        return Err("The template backup is encrypted. Create an unencrypted local iPhone backup for offline conversion.".to_owned());
    }
    let database =
        Connection::open_with_flags(&chat_storage, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|error| error.to_string())?;
    let tables = table_names(&database)?;
    for required in ["ZWACHATSESSION", "ZWAMESSAGE", "Z_PRIMARYKEY"] {
        if !tables.contains(required) {
            return Err(format!(
                "Unsupported ChatStorage schema: missing {required}"
            ));
        }
    }
    Ok(IosTemplate {
        backup_root: root.to_path_buf(),
        file_id,
        domain,
        relative_path,
        chat_storage,
    })
}

pub fn prepare_restore(
    archive: &Path,
    template_root: &Path,
    output_root: &Path,
    owner_jid: &str,
    device_udid: &str,
) -> Result<PrepareResult, String> {
    if owner_jid.trim().is_empty() || !owner_jid.contains('@') {
        return Err(
            "Enter the destination WhatsApp account JID, for example 15551234567@s.whatsapp.net"
                .to_owned(),
        );
    }
    if output_root.exists() {
        let _ = fs::remove_dir_all(output_root);
    }
    let source_template = inspect_template(template_root)?;
    // idevicebackup2 restore expects: <dir>/<UDID>/Manifest.db
    // So we must copy the template into output_root/<UDID>/
    let udid_dir = output_root.join(device_udid);
    copy_tree(template_root, &udid_dir)?;
    let working_template = inspect_template(&udid_dir)?;
    let original = working_template
        .chat_storage
        .with_extension("sqlite.icarefone-original");
    fs::copy(&working_template.chat_storage, &original).map_err(|error| error.to_string())?;
    let (conversations_written, messages_written) =
        import_archive(archive, &working_template.chat_storage, owner_jid)?;

    let connection =
        Connection::open(&working_template.chat_storage).map_err(|error| error.to_string())?;
    connection
        .execute_batch("PRAGMA journal_mode=DELETE; PRAGMA wal_checkpoint(TRUNCATE);")
        .map_err(|error| error.to_string())?;
    let integrity: String = connection
        .query_row("PRAGMA integrity_check", [], |row| row.get(0))
        .map_err(|error| error.to_string())?;
    if integrity != "ok" {
        return Err(format!(
            "Generated ChatStorage failed integrity check: {integrity}"
        ));
    }
    drop(connection);

    // Remove any stray journal files in the backup directory
    let file_id = &working_template.file_id;
    if let Some(parent) = working_template.chat_storage.parent() {
        let _ = fs::remove_file(parent.join(format!("{file_id}-wal")));
        let _ = fs::remove_file(parent.join(format!("{file_id}-shm")));
        let _ = fs::remove_file(parent.join(format!("{file_id}.sqlite.icarefone-original-wal")));
        let _ = fs::remove_file(parent.join(format!("{file_id}.sqlite.icarefone-original-shm")));
    }

    // CRITICAL: Update Manifest.db so iOS MobileBackup2 knows the true restored file size!
    let final_size = fs::metadata(&working_template.chat_storage)
        .map_err(|error| error.to_string())?
        .len();
    let manifest_db = udid_dir.join("Manifest.db");
    update_manifest_file_size(&manifest_db, file_id, final_size)?;

    let result = PrepareResult {
        output_backup: output_root.to_path_buf(),
        conversations_written,
        messages_written,
        warnings: vec![
            "Media files remain staged beside the normalized archive until this ChatStorage schema exposes a validated media-item mapping.".to_owned(),
            "Always test the restored backup safely before relying on it.".to_owned(),
        ],
    };
    let report = serde_json::to_vec_pretty(&result).map_err(|error| error.to_string())?;
    fs::write(output_root.join("icarefone-restore-ready.json"), report)
        .map_err(|error| error.to_string())?;
    let _ = source_template;
    Ok(result)
}

pub fn update_manifest_file_size(manifest_db: &Path, file_id: &str, new_size: u64) -> Result<(), String> {
    let conn = Connection::open(manifest_db).map_err(|e| format!("Could not open Manifest.db: {e}"))?;
    let blob: Vec<u8> = conn
        .query_row("SELECT file FROM Files WHERE fileID=?1", [file_id], |row| row.get(0))
        .map_err(|e| format!("Could not query file blob for {file_id}: {e}"))?;

    let mut val = plist::Value::from_reader(std::io::Cursor::new(&blob))
        .map_err(|e| format!("Could not parse file plist blob for {file_id}: {e}"))?;

    if let Some(dict) = val.as_dictionary_mut() {
        if let Some(objects) = dict.get_mut("$objects").and_then(|v| v.as_array_mut()) {
            for obj in objects.iter_mut() {
                if let Some(file_dict) = obj.as_dictionary_mut() {
                    if file_dict.contains_key("Size") {
                        file_dict.insert("Size".to_string(), plist::Value::Integer((new_size as i64).into()));
                        break;
                    }
                }
            }
        }
    }

    let mut new_blob = Vec::new();
    plist::to_writer_binary(&mut new_blob, &val)
        .map_err(|e| format!("Could not serialize updated plist blob for {file_id}: {e}"))?;

    conn.execute(
        "UPDATE Files SET file=?1 WHERE fileID=?2",
        params![new_blob, file_id],
    )
    .map_err(|e| format!("Could not update Manifest.db: {e}"))?;

    conn.execute_batch("PRAGMA journal_mode = DELETE; PRAGMA wal_checkpoint(TRUNCATE);")
        .map_err(|e| e.to_string())?;

    Ok(())
}

pub fn detect_owner_jid(root: &Path) -> Option<String> {
    let manifest = root.join("Manifest.db");
    if !manifest.is_file() {
        return None;
    }
    let conn = Connection::open_with_flags(&manifest, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).ok()?;
    let path: String = conn
        .query_row(
            "SELECT relativePath FROM Files WHERE relativePath LIKE 'Library/Mobile Documents/%/Accounts/%/backup/%' LIMIT 1",
            [],
            |r| r.get(0),
        )
        .ok()?;

    if let Some(start) = path.find("/Accounts/") {
        let rest = &path[start + "/Accounts/".len()..];
        if let Some(end) = rest.find('/') {
            let number = &rest[..end];
            if !number.is_empty() && number.chars().all(|c| c.is_ascii_digit()) {
                return Some(format!("{number}@s.whatsapp.net"));
            }
        }
    }
    None
}

fn import_archive(
    archive: &Path,
    chat_storage: &Path,
    owner_jid: &str,
) -> Result<(u64, u64), String> {
    let source = Connection::open_with_flags(archive, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|error| format!("Could not open normalized archive: {error}"))?;
    let mut target = Connection::open(chat_storage)
        .map_err(|error| format!("Could not open ChatStorage: {error}"))?;
    target
        .execute_batch("PRAGMA foreign_keys=OFF; PRAGMA journal_mode=DELETE;")
        .map_err(|error| error.to_string())?;
    let chat_columns = columns(&target, "ZWACHATSESSION")?;
    let message_columns = columns(&target, "ZWAMESSAGE")?;
    for required in ["Z_PK", "Z_ENT", "Z_OPT", "ZCONTACTJID"] {
        if !chat_columns.contains(required) {
            return Err(format!(
                "Unsupported ChatStorage chat schema: missing {required}"
            ));
        }
    }
    for required in [
        "Z_PK",
        "Z_ENT",
        "Z_OPT",
        "ZCHATSESSION",
        "ZISFROMME",
        "ZMESSAGEDATE",
    ] {
        if !message_columns.contains(required) {
            return Err(format!(
                "Unsupported ChatStorage message schema: missing {required}"
            ));
        }
    }
    let chat_entity = entity_id(&target, "ChatSession")?;
    let message_entity = entity_id(&target, "Message")?;
    let transaction = target.transaction().map_err(|error| error.to_string())?;
    let mut next_chat: i64 = transaction
        .query_row(
            "SELECT COALESCE(MAX(Z_PK),0)+1 FROM ZWACHATSESSION",
            [],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    let mut next_message: i64 = transaction
        .query_row(
            "SELECT COALESCE(MAX(Z_PK),0)+1 FROM ZWAMESSAGE",
            [],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    let mut chat_map = HashMap::new();
    let mut conversations_written = 0;
    let mut chats = source
        .prepare("SELECT id,jid,title FROM conversation ORDER BY id")
        .map_err(|error| error.to_string())?;
    let rows = chats
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
            ))
        })
        .map_err(|error| error.to_string())?;
    for row in rows {
        let (archive_id, jid, title) = row.map_err(|error| error.to_string())?;
        let existing: Option<i64> = transaction
            .query_row(
                "SELECT Z_PK FROM ZWACHATSESSION WHERE ZCONTACTJID=?1 LIMIT 1",
                [&jid],
                |value| value.get(0),
            )
            .optional()
            .map_err(|error| error.to_string())?;
        let pk = if let Some(existing) = existing {
            existing
        } else {
            let mut values = vec![
                ("Z_PK", Value::Integer(next_chat)),
                ("Z_ENT", Value::Integer(chat_entity)),
                ("Z_OPT", Value::Integer(1)),
                ("ZCONTACTJID", Value::Text(jid.clone())),
            ];
            if chat_columns.contains("ZPARTNERNAME") {
                values.push((
                    "ZPARTNERNAME",
                    title.map(Value::Text).unwrap_or(Value::Null),
                ));
            }
            if chat_columns.contains("ZSESSIONTYPE") {
                let session_type = if jid.ends_with("@g.us") { 1 } else { 0 };
                values.push(("ZSESSIONTYPE", Value::Integer(session_type)));
            }
            insert_dynamic(&transaction, "ZWACHATSESSION", values)?;
            let current = next_chat;
            next_chat += 1;
            conversations_written += 1;
            current
        };
        chat_map.insert(archive_id, (pk, jid));
    }

    let mut messages_written = 0;
    let mut messages = source
        .prepare(
            "SELECT source_id,conversation_id,stanza_id,sender_jid,from_me,status,timestamp_ms,
                message_type,text,starred FROM message ORDER BY timestamp_ms,source_id",
        )
        .map_err(|error| error.to_string())?;
    let mut rows = messages.query([]).map_err(|error| error.to_string())?;
    while let Some(row) = rows.next().map_err(|error| error.to_string())? {
        let conversation_id: i64 = row.get(1).map_err(|error| error.to_string())?;
        let Some((chat_pk, chat_jid)) = chat_map.get(&conversation_id) else {
            continue;
        };
        let from_me = row.get::<_, i64>(4).unwrap_or(0);
        let sender = row.get::<_, Option<String>>(3).ok().flatten();
        let from_jid = if from_me != 0 {
            owner_jid.to_owned()
        } else {
            sender.unwrap_or_else(|| chat_jid.clone())
        };
        let to_jid = if from_me != 0 {
            chat_jid.clone()
        } else {
            owner_jid.to_owned()
        };
        let timestamp_ms = row.get::<_, Option<i64>>(6).ok().flatten().unwrap_or(0);
        let mut values = vec![
            ("Z_PK", Value::Integer(next_message)),
            ("Z_ENT", Value::Integer(message_entity)),
            ("Z_OPT", Value::Integer(1)),
            ("ZCHATSESSION", Value::Integer(*chat_pk)),
            ("ZISFROMME", Value::Integer(from_me)),
            (
                "ZMESSAGEDATE",
                Value::Real(timestamp_ms as f64 / 1000.0 - IOS_EPOCH_OFFSET),
            ),
        ];
        optional_value(
            &mut values,
            &message_columns,
            "ZSTANZAID",
            row.get::<_, Option<String>>(2)
                .ok()
                .flatten()
                .map(Value::Text),
        );
        optional_value(
            &mut values,
            &message_columns,
            "ZFROMJID",
            Some(Value::Text(from_jid)),
        );
        optional_value(
            &mut values,
            &message_columns,
            "ZTOJID",
            Some(Value::Text(to_jid)),
        );
        optional_value(
            &mut values,
            &message_columns,
            "ZMESSAGESTATUS",
            row.get::<_, Option<i64>>(5)
                .ok()
                .flatten()
                .map(Value::Integer),
        );
        optional_value(
            &mut values,
            &message_columns,
            "ZMESSAGETYPE",
            row.get::<_, Option<i64>>(7)
                .ok()
                .flatten()
                .map(Value::Integer),
        );
        optional_value(
            &mut values,
            &message_columns,
            "ZTEXT",
            row.get::<_, Option<String>>(8)
                .ok()
                .flatten()
                .map(Value::Text),
        );
        optional_value(
            &mut values,
            &message_columns,
            "ZSTARRED",
            Some(Value::Integer(row.get::<_, i64>(9).unwrap_or(0))),
        );
        insert_dynamic(&transaction, "ZWAMESSAGE", values)?;
        next_message += 1;
        messages_written += 1;
    }
    transaction
        .execute(
            "UPDATE Z_PRIMARYKEY SET Z_MAX=?1 WHERE Z_ENT=?2",
            params![next_chat - 1, chat_entity],
        )
        .map_err(|error| error.to_string())?;
    transaction
        .execute(
            "UPDATE Z_PRIMARYKEY SET Z_MAX=?1 WHERE Z_ENT=?2",
            params![next_message - 1, message_entity],
        )
        .map_err(|error| error.to_string())?;

    // Update ZWACHATSESSION metadata so iOS WhatsApp displays all active chats in the chat list
    transaction.execute_batch(
        "UPDATE ZWACHATSESSION SET
            ZHIDDEN = 0,
            ZARCHIVED = COALESCE(ZARCHIVED, 0),
            ZMESSAGECOUNTER = (SELECT COUNT(*) FROM ZWAMESSAGE WHERE ZWAMESSAGE.ZCHATSESSION = ZWACHATSESSION.Z_PK),
            ZLASTMESSAGE = (SELECT Z_PK FROM ZWAMESSAGE WHERE ZWAMESSAGE.ZCHATSESSION = ZWACHATSESSION.Z_PK ORDER BY ZMESSAGEDATE DESC, Z_PK DESC LIMIT 1),
            ZLASTMESSAGEDATE = (SELECT ZMESSAGEDATE FROM ZWAMESSAGE WHERE ZWAMESSAGE.ZCHATSESSION = ZWACHATSESSION.Z_PK ORDER BY ZMESSAGEDATE DESC, Z_PK DESC LIMIT 1),
            ZLASTMESSAGETEXT = (SELECT ZTEXT FROM ZWAMESSAGE WHERE ZWAMESSAGE.ZCHATSESSION = ZWACHATSESSION.Z_PK ORDER BY ZMESSAGEDATE DESC, Z_PK DESC LIMIT 1)
        WHERE Z_PK IN (SELECT DISTINCT ZCHATSESSION FROM ZWAMESSAGE);"
    ).map_err(|error| format!("Failed to update chat session metadata: {error}"))?;

    transaction.commit().map_err(|error| error.to_string())?;
    Ok((conversations_written, messages_written))
}

fn insert_dynamic(
    connection: &Connection,
    table: &str,
    values: Vec<(&str, Value)>,
) -> Result<(), String> {
    let names = values
        .iter()
        .map(|(name, _)| *name)
        .collect::<Vec<_>>()
        .join(",");
    let placeholders = (1..=values.len())
        .map(|index| format!("?{index}"))
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!("INSERT INTO {table}({names}) VALUES({placeholders})");
    connection
        .execute(
            &sql,
            params_from_iter(values.into_iter().map(|(_, value)| value)),
        )
        .map_err(|error| error.to_string())?;
    Ok(())
}

fn optional_value(
    values: &mut Vec<(&'static str, Value)>,
    columns: &HashSet<String>,
    name: &'static str,
    value: Option<Value>,
) {
    if columns.contains(name) {
        values.push((name, value.unwrap_or(Value::Null)));
    }
}

fn entity_id(connection: &Connection, suffix: &str) -> Result<i64, String> {
    connection
        .query_row(
            "SELECT Z_ENT FROM Z_PRIMARYKEY WHERE Z_NAME LIKE ?1 ORDER BY length(Z_NAME) LIMIT 1",
            [format!("%{suffix}")],
            |row| row.get(0),
        )
        .map_err(|error| format!("Could not find Core Data entity {suffix}: {error}"))
}

fn columns(connection: &Connection, table: &str) -> Result<HashSet<String>, String> {
    let mut statement = connection
        .prepare(&format!("PRAGMA table_info({table})"))
        .map_err(|error| error.to_string())?;
    statement
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(|error| error.to_string())?
        .collect::<Result<HashSet<_>, _>>()
        .map_err(|error| error.to_string())
}

fn table_names(connection: &Connection) -> Result<HashSet<String>, String> {
    let mut statement = connection
        .prepare("SELECT name FROM sqlite_master WHERE type='table'")
        .map_err(|error| error.to_string())?;
    statement
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|error| error.to_string())?
        .collect::<Result<HashSet<_>, _>>()
        .map_err(|error| error.to_string())
}

fn copy_tree(source: &Path, destination: &Path) -> Result<(), String> {
    for entry in WalkDir::new(source)
        .follow_links(false)
        .into_iter()
        .filter_map(Result::ok)
    {
        let relative = entry
            .path()
            .strip_prefix(source)
            .map_err(|error| error.to_string())?;
        let target = destination.join(relative);
        if entry.file_type().is_dir() {
            fs::create_dir_all(&target).map_err(|error| error.to_string())?;
        } else if entry.file_type().is_file() {
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent).map_err(|error| error.to_string())?;
            }
            fs::copy(entry.path(), target).map_err(|error| error.to_string())?;
        }
    }
    Ok(())
}

pub fn backup_file_id(domain: &str, relative_path: &str) -> String {
    let mut hasher = Sha1::new();
    hasher.update(format!("{domain}-{relative_path}"));
    hex::encode(hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apple_backup_file_id_is_deterministic() {
        assert_eq!(
            backup_file_id("HomeDomain", "Library/test"),
            "9ca7531467c9bdcc35bb9c155bb0c8af9abe4db4"
        );
    }

    #[test]
    fn test_update_mbfile_size() {
        let bytes = std::fs::read("/tmp/test_mbfile.plist").unwrap();
        let mut val = plist::Value::from_reader(std::io::Cursor::new(bytes)).unwrap();
        let dict = val.as_dictionary_mut().unwrap();
        let objects = dict.get_mut("$objects").unwrap().as_array_mut().unwrap();
        let file_dict = objects[1].as_dictionary_mut().unwrap();
        file_dict.insert("Size".to_string(), plist::Value::Integer(139603968.into()));
        let mut buf = Vec::new();
        plist::to_writer_binary(&mut buf, &val).unwrap();
        let roundtrip = plist::Value::from_reader(std::io::Cursor::new(&buf)).unwrap();
        let r_size = roundtrip.as_dictionary().unwrap().get("$objects").unwrap().as_array().unwrap()[1].as_dictionary().unwrap().get("Size").unwrap().as_signed_integer().unwrap();
        assert_eq!(r_size, 139603968);
    }
}

