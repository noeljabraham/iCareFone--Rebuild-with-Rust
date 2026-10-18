pub mod archive;
pub mod crypto;
pub mod inventory;
pub mod ios_backup;

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TransferReport {
    pub source_database: PathBuf,
    pub decrypted_database: PathBuf,
    pub media_root: Option<PathBuf>,
    pub format: String,
    pub chats: u64,
    pub messages: u64,
    pub media_files: u64,
    pub warnings: Vec<String>,
}

pub fn inspect_and_decrypt(
    source: &Path,
    key: Option<&str>,
    workspace: &Path,
) -> Result<TransferReport, String> {
    let inventory = inventory::inspect(source)?;
    std::fs::create_dir_all(workspace).map_err(|error| error.to_string())?;
    let decrypted = workspace.join("msgstore.db");
    if decrypted.exists() {
        let _ = std::fs::remove_file(&decrypted);
    }
    let format = inventory.format.label().to_owned();
    if inventory.format == inventory::BackupFormat::Sqlite {
        std::fs::copy(&inventory.database, &decrypted).map_err(|error| error.to_string())?;
    } else {
        let key_str = key
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| format!("A user-supplied {} key is required (paste 64-char hex key or file path)", format))?;
        crypto::decrypt_file(&inventory.database, key_str, &decrypted)?;
    }
    let summary = archive::inspect_android_database(&decrypted)?;
    Ok(TransferReport {
        source_database: inventory.database,
        decrypted_database: decrypted,
        media_root: inventory.media_root,
        format,
        chats: summary.chats,
        messages: summary.messages,
        media_files: summary.media_files,
        warnings: summary.warnings,
    })
}

