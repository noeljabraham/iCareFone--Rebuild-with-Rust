use std::fs;
use std::path::{Path, PathBuf};

use walkdir::WalkDir;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BackupFormat {
    Sqlite,
    Crypt12,
    Crypt14,
    Crypt15,
}

impl BackupFormat {
    pub fn label(self) -> &'static str {
        match self {
            Self::Sqlite => "decrypted SQLite",
            Self::Crypt12 => "Crypt12",
            Self::Crypt14 => "Crypt14",
            Self::Crypt15 => "Crypt15",
        }
    }

    fn priority(self) -> u8 {
        match self {
            Self::Sqlite => 4,
            Self::Crypt15 => 3,
            Self::Crypt14 => 2,
            Self::Crypt12 => 1,
        }
    }
}

#[derive(Clone, Debug)]
pub struct SourceInventory {
    pub database: PathBuf,
    pub media_root: Option<PathBuf>,
    pub format: BackupFormat,
}

pub fn inspect(source: &Path) -> Result<SourceInventory, String> {
    if !source.is_absolute() {
        return Err("WhatsApp source must be an absolute path".to_owned());
    }
    if !source.exists() {
        return Err("WhatsApp source does not exist".to_owned());
    }
    let mut candidates = Vec::new();
    let mut media_root = None;
    let entries: Box<dyn Iterator<Item = PathBuf>> = if source.is_file() {
        Box::new(std::iter::once(source.to_path_buf()))
    } else {
        Box::new(
            WalkDir::new(source)
                .follow_links(false)
                .max_depth(6)
                .into_iter()
                .filter_map(Result::ok)
                .map(|entry| entry.into_path()),
        )
    };
    for path in entries {
        if path.is_dir()
            && path
                .file_name()
                .is_some_and(|name| name.eq_ignore_ascii_case("Media"))
        {
            media_root.get_or_insert(path);
            continue;
        }
        if !path.is_file() {
            continue;
        }
        if let Some(format) = identify(&path) {
            let modified = fs::metadata(&path).and_then(|value| value.modified()).ok();
            candidates.push((format, modified, path));
        }
    }
    candidates.sort_by(|left, right| {
        right
            .0
            .priority()
            .cmp(&left.0.priority())
            .then_with(|| right.1.cmp(&left.1))
    });
    let Some((format, _, database)) = candidates.into_iter().next() else {
        return Err("No msgstore.db or msgstore.db.crypt12/14/15 file was found".to_owned());
    };
    Ok(SourceInventory {
        database,
        media_root,
        format,
    })
}

fn identify(path: &Path) -> Option<BackupFormat> {
    let name = path.file_name()?.to_string_lossy().to_ascii_lowercase();
    if name.ends_with(".crypt15") {
        Some(BackupFormat::Crypt15)
    } else if name.ends_with(".crypt14") {
        Some(BackupFormat::Crypt14)
    } else if name.ends_with(".crypt12") {
        Some(BackupFormat::Crypt12)
    } else if name == "msgstore.db" {
        Some(BackupFormat::Sqlite)
    } else {
        None
    }
}

