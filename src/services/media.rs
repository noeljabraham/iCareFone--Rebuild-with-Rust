use std::fs;
use std::path::Path;

use crate::domain::MediaFile;

use super::command;

pub fn mount(mount_point: &Path, udid: &str) -> Result<String, String> {
    command::ensure_local_path(mount_point)?;
    if !command::available("ifuse") {
        return Err("Media mounting requires `ifuse`. Install via Homebrew: `brew install ifuse`".to_owned());
    }
    fs::create_dir_all(mount_point)
        .map_err(|error| format!("Could not create mount directory: {error}"))?;
    let point = mount_point.to_string_lossy().to_string();
    command::checked("ifuse", ["--udid", udid, "--camera", &point])?;
    Ok(format!("Camera media mounted at {point}"))
}

pub fn unmount(mount_point: &Path) -> Result<String, String> {
    command::ensure_local_path(mount_point)?;
    let point = mount_point.to_string_lossy().to_string();
    if command::available("diskutil") {
        command::checked("diskutil", ["unmount", &point])?;
    } else {
        command::checked("umount", [&point])?;
    }
    Ok(format!("Unmounted {point}"))
}

pub fn list_media(mount_point: &Path) -> Vec<MediaFile> {
    let mut files = Vec::new();
    let Ok(entries) = walkdir::WalkDir::new(mount_point)
        .max_depth(4)
        .into_iter()
        .collect::<Result<Vec<_>, _>>() else {
        return files;
    };

    for entry in entries {
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path().to_path_buf();
        let name = entry.file_name().to_string_lossy().to_string();
        let ext = path.extension().unwrap_or_default().to_string_lossy().to_lowercase();
        let is_video = matches!(ext.as_str(), "mov" | "mp4" | "m4v");
        let is_image = matches!(ext.as_str(), "jpg" | "jpeg" | "png" | "heic" | "dng" | "gif");

        if is_image || is_video {
            let size_bytes = entry.metadata().map(|m| m.len()).unwrap_or(0);
            files.push(MediaFile {
                name,
                path,
                size_bytes,
                is_video,
            });
        }
    }
    files.sort_by(|a, b| a.name.cmp(&b.name));
    files
}

pub fn copy_tree(source: &Path, destination: &Path) -> Result<String, String> {
    command::ensure_local_path(source)?;
    command::ensure_local_path(destination)?;
    if !source.is_dir() {
        return Err("Mounted media source is not available. Please mount the camera first.".to_owned());
    }
    fs::create_dir_all(destination)
        .map_err(|error| format!("Could not create destination: {error}"))?;

    let mut copied = 0_u64;
    let mut pending = vec![(source.to_path_buf(), destination.to_path_buf())];
    while let Some((from, to)) = pending.pop() {
        fs::create_dir_all(&to).map_err(|error| error.to_string())?;
        for entry in fs::read_dir(&from)
            .map_err(|error| error.to_string())?
            .flatten()
        {
            let target = to.join(entry.file_name());
            if entry.path().is_dir() {
                pending.push((entry.path(), target));
            } else {
                fs::copy(entry.path(), target).map_err(|error| error.to_string())?;
                copied += 1;
            }
        }
    }
    Ok(format!(
        "Successfully exported {copied} photos and videos to {}",
        destination.display()
    ))
}

