#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use walkdir::WalkDir;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DatabaseSummary {
    pub chats: u64,
    pub messages: u64,
    pub media_files: u64,
    pub warnings: Vec<String>,
}

pub fn inspect_android_database(path: &Path) -> Result<DatabaseSummary, String> {
    let connection = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|error| format!("Could not open WhatsApp database: {error}"))?;
    connection
        .pragma_update(None, "query_only", true)
        .map_err(|error| error.to_string())?;
    let tables = table_names(&connection)?;
    let chats = count_first(&connection, &tables, &["chat", "chats"])?;
    let messages = count_first(&connection, &tables, &["message", "messages"])?;
    let media_files = count_first(&connection, &tables, &["message_media", "media_refs"])?;
    let mut warnings = Vec::new();
    if !tables.contains(&"jid".to_owned()) {
        warnings.push(
            "No modern jid table was found; the legacy schema adapter will be used".to_owned(),
        );
    }
    connection
        .query_row("PRAGMA integrity_check", [], |row| row.get::<_, String>(0))
        .map_err(|error| error.to_string())
        .and_then(|result| {
            if result == "ok" {
                Ok(())
            } else {
                Err(format!("SQLite integrity check failed: {result}"))
            }
        })?;
    Ok(DatabaseSummary {
        chats,
        messages,
        media_files,
        warnings,
    })
}

pub fn build_normalized_archive(
    source_database: &Path,
    media_root: Option<&Path>,
    destination: &Path,
) -> Result<DatabaseSummary, String> {
    if destination.exists() {
        let _ = fs::remove_file(destination);
    }
    let source =
        Connection::open_with_flags(source_database, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|error| error.to_string())?;
    source
        .pragma_update(None, "query_only", true)
        .map_err(|error| error.to_string())?;
    let tables = table_names(&source)?;
    let mut target = Connection::open(destination).map_err(|error| error.to_string())?;
    target
        .execute_batch(
            "PRAGMA journal_mode=WAL;
         PRAGMA foreign_keys=ON;
         CREATE TABLE metadata(key TEXT PRIMARY KEY, value TEXT NOT NULL);
         CREATE TABLE conversation(
           id INTEGER PRIMARY KEY, source_id INTEGER NOT NULL, jid TEXT NOT NULL UNIQUE,
           title TEXT, is_group INTEGER NOT NULL DEFAULT 0
         );
         CREATE TABLE message(
           id INTEGER PRIMARY KEY, source_id INTEGER NOT NULL UNIQUE,
           conversation_id INTEGER NOT NULL REFERENCES conversation(id),
           stanza_id TEXT, sender_jid TEXT, from_me INTEGER NOT NULL,
           status INTEGER, timestamp_ms INTEGER, message_type INTEGER,
           text TEXT, starred INTEGER NOT NULL DEFAULT 0,
           media_path TEXT, media_mime TEXT, media_name TEXT, media_size INTEGER
         );
         CREATE INDEX message_conversation_time ON message(conversation_id, timestamp_ms);",
        )
        .map_err(|error| error.to_string())?;
    target
        .execute(
            "INSERT INTO metadata VALUES('format','icarefone-whatsapp-1')",
            [],
        )
        .map_err(|error| error.to_string())?;

    if tables.contains(&"message".to_owned())
        && tables.contains(&"chat".to_owned())
        && tables.contains(&"jid".to_owned())
    {
        import_modern(&source, &mut target, &tables)?;
    } else if tables.contains(&"messages".to_owned()) {
        import_legacy(&source, &mut target)?;
    } else {
        return Err(
            "Unsupported Android WhatsApp schema: neither message nor messages table exists"
                .to_owned(),
        );
    }

    let package_media = destination.with_file_name("Media");
    let media_files = if let Some(media_root) = media_root {
        copy_media(media_root, &package_media)?
    } else {
        0
    };
    let chats = target
        .query_row("SELECT count(*) FROM conversation", [], |row| row.get(0))
        .map_err(|error| error.to_string())?;
    let messages = target
        .query_row("SELECT count(*) FROM message", [], |row| row.get(0))
        .map_err(|error| error.to_string())?;
    target
        .execute(
            "INSERT INTO metadata VALUES('media_files',?1)",
            [media_files.to_string()],
        )
        .map_err(|error| error.to_string())?;
    target
        .execute_batch("PRAGMA wal_checkpoint(TRUNCATE); PRAGMA optimize;")
        .map_err(|error| error.to_string())?;
    Ok(DatabaseSummary {
        chats,
        messages,
        media_files,
        warnings: Vec::new(),
    })
}

fn import_modern(
    source: &Connection,
    target: &mut Connection,
    tables: &[String],
) -> Result<(), String> {
    let transaction = target.transaction().map_err(|error| error.to_string())?;
    {
        let mut chats = source
            .prepare(
                "SELECT c._id, COALESCE(j.raw_string, j.user || '@' || j.server), c.subject
             FROM chat c JOIN jid j ON j._id=c.jid_row_id ORDER BY c._id",
            )
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
            let (source_id, jid, title) = row.map_err(|error| error.to_string())?;
            transaction.execute(
                "INSERT OR IGNORE INTO conversation(source_id,jid,title,is_group) VALUES(?1,?2,?3,?4)",
                params![source_id, jid, title, i64::from(jid.ends_with("@g.us"))],
            ).map_err(|error| error.to_string())?;
        }
    }
    let media_columns = if tables.contains(&"message_media".to_owned()) {
        column_names(source, "message_media")?
    } else {
        Vec::new()
    };
    let media_join = if !media_columns.is_empty() {
        "LEFT JOIN message_media mm ON mm.message_row_id=m._id"
    } else {
        "LEFT JOIN (SELECT NULL message_row_id, NULL file_path, NULL mime_type, NULL media_name, NULL file_size) mm ON 0"
    };
    let media_size = match (
        media_columns.iter().any(|column| column == "file_size"),
        media_columns.iter().any(|column| column == "file_length"),
    ) {
        (true, true) => "COALESCE(mm.file_size, mm.file_length)",
        (true, false) => "mm.file_size",
        (false, true) => "mm.file_length",
        (false, false) => "NULL",
    };
    let sql = format!(
        "SELECT m._id, m.chat_row_id, m.key_id,
                CASE WHEN sj.raw_string IS NOT NULL THEN sj.raw_string
                     WHEN sj.user IS NOT NULL THEN sj.user || '@' || sj.server ELSE NULL END,
                m.from_me, m.status, m.timestamp, m.message_type, m.text_data, m.starred,
                mm.file_path, mm.mime_type, mm.media_name, {media_size}
         FROM message m
         LEFT JOIN jid sj ON sj._id=m.sender_jid_row_id
         {media_join} ORDER BY m._id"
    );
    let mut statement = source.prepare(&sql).map_err(|error| error.to_string())?;
    let mut rows = statement.query([]).map_err(|error| error.to_string())?;
    while let Some(row) = rows.next().map_err(|error| error.to_string())? {
        let chat_source: i64 = row.get(1).map_err(|error| error.to_string())?;
        let conversation: Option<i64> = transaction
            .query_row(
                "SELECT id FROM conversation WHERE source_id=?1",
                [chat_source],
                |value| value.get(0),
            )
            .optional()
            .map_err(|error| error.to_string())?;
        let Some(conversation) = conversation else {
            continue;
        };
        transaction
            .execute(
                "INSERT OR IGNORE INTO message(
               source_id,conversation_id,stanza_id,sender_jid,from_me,status,timestamp_ms,
               message_type,text,starred,media_path,media_mime,media_name,media_size
             ) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14)",
                params![
                    row.get::<_, i64>(0).map_err(|error| error.to_string())?,
                    conversation,
                    row.get::<_, Option<String>>(2).ok().flatten(),
                    row.get::<_, Option<String>>(3).ok().flatten(),
                    row.get::<_, i64>(4).unwrap_or(0),
                    row.get::<_, Option<i64>>(5).ok().flatten(),
                    row.get::<_, Option<i64>>(6).ok().flatten(),
                    row.get::<_, Option<i64>>(7).ok().flatten(),
                    row.get::<_, Option<String>>(8).ok().flatten(),
                    row.get::<_, i64>(9).unwrap_or(0),
                    row.get::<_, Option<String>>(10).ok().flatten(),
                    row.get::<_, Option<String>>(11).ok().flatten(),
                    row.get::<_, Option<String>>(12).ok().flatten(),
                    row.get::<_, Option<i64>>(13).ok().flatten(),
                ],
            )
            .map_err(|error| error.to_string())?;
    }
    transaction.commit().map_err(|error| error.to_string())
}

fn import_legacy(source: &Connection, target: &mut Connection) -> Result<(), String> {
    let transaction = target.transaction().map_err(|error| error.to_string())?;
    let mut statement = source.prepare(
        "SELECT _id,key_remote_jid,key_id,NULL,key_from_me,status,timestamp,media_wa_type,data,starred,
                media_name,media_mime_type,media_name,media_size FROM messages ORDER BY _id",
    ).map_err(|error| format!("Unsupported legacy messages schema: {error}"))?;
    let mut rows = statement.query([]).map_err(|error| error.to_string())?;
    while let Some(row) = rows.next().map_err(|error| error.to_string())? {
        let jid: String = row.get(1).map_err(|error| error.to_string())?;
        transaction
            .execute(
                "INSERT OR IGNORE INTO conversation(source_id,jid,is_group) VALUES(?1,?2,?3)",
                params![
                    row.get::<_, i64>(0).unwrap_or(0),
                    jid,
                    i64::from(jid.ends_with("@g.us"))
                ],
            )
            .map_err(|error| error.to_string())?;
        let conversation: i64 = transaction
            .query_row(
                "SELECT id FROM conversation WHERE jid=?1",
                [&jid],
                |value| value.get(0),
            )
            .map_err(|error| error.to_string())?;
        transaction.execute(
            "INSERT OR IGNORE INTO message(source_id,conversation_id,stanza_id,sender_jid,from_me,status,
             timestamp_ms,message_type,text,starred,media_path,media_mime,media_name,media_size)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14)",
            params![row.get::<_, i64>(0).unwrap_or(0), conversation, row.get::<_, Option<String>>(2).ok().flatten(),
                row.get::<_, Option<String>>(3).ok().flatten(), row.get::<_, i64>(4).unwrap_or(0),
                row.get::<_, Option<i64>>(5).ok().flatten(), row.get::<_, Option<i64>>(6).ok().flatten(),
                row.get::<_, Option<i64>>(7).ok().flatten(), row.get::<_, Option<String>>(8).ok().flatten(),
                row.get::<_, i64>(9).unwrap_or(0), row.get::<_, Option<String>>(10).ok().flatten(),
                row.get::<_, Option<String>>(11).ok().flatten(), row.get::<_, Option<String>>(12).ok().flatten(),
                row.get::<_, Option<i64>>(13).ok().flatten()],
        ).map_err(|error| error.to_string())?;
    }
    transaction.commit().map_err(|error| error.to_string())
}

fn copy_media(source: &Path, destination: &Path) -> Result<u64, String> {
    let mut count = 0;
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
            count += 1;
        }
    }
    Ok(count)
}

fn table_names(connection: &Connection) -> Result<Vec<String>, String> {
    let mut statement = connection
        .prepare("SELECT name FROM sqlite_master WHERE type='table'")
        .map_err(|error| error.to_string())?;
    statement
        .query_map([], |row| row.get(0))
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<String>, _>>()
        .map_err(|error| error.to_string())
}

fn column_names(connection: &Connection, table: &str) -> Result<Vec<String>, String> {
    let mut statement = connection
        .prepare(&format!("PRAGMA table_info(\"{table}\")"))
        .map_err(|error| error.to_string())?;
    statement
        .query_map([], |row| row.get(1))
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<String>, _>>()
        .map_err(|error| error.to_string())
}

fn count_first(
    connection: &Connection,
    tables: &[String],
    candidates: &[&str],
) -> Result<u64, String> {
    for candidate in candidates {
        if tables.iter().any(|table| table == candidate) {
            return connection
                .query_row(
                    &format!("SELECT count(*) FROM \"{candidate}\""),
                    [],
                    |row| row.get(0),
                )
                .map_err(|error| error.to_string());
        }
    }
    Ok(0)
}

pub fn archive_path(workspace: &Path) -> PathBuf {
    workspace.join("icarefone-whatsapp.sqlite")
}

