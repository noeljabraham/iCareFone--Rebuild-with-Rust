use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const ALLOWED_TOOLS: &[&str] = &[
    "idevice_id",
    "ideviceinfo",
    "idevicebackup2",
    "idevicediagnostics",
    "idevicescreenshot",
    "idevicecrashreport",
    "idevicesyslog",
    "ideviceinstaller",
    "ifuse",
    "umount",
    "diskutil",
    "adb",
];

pub fn tool_search_paths() -> Vec<PathBuf> {
    let mut paths = vec![
        PathBuf::from("/opt/homebrew/bin"),
        PathBuf::from("/opt/homebrew/sbin"),
        PathBuf::from("/usr/local/bin"),
        PathBuf::from("/usr/bin"),
        PathBuf::from("/bin"),
        PathBuf::from("/usr/sbin"),
        PathBuf::from("/sbin"),
    ];
    if let Ok(home) = std::env::var("HOME") {
        let home_path = PathBuf::from(home);
        paths.push(home_path.join("Library/Android/sdk/platform-tools"));
        paths.push(home_path.join(".cargo/bin"));
        paths.push(home_path.join("Library/Android/sdk/cmdline-tools/latest/bin"));
    }
    // Also hardcode standard macOS user SDK path if HOME wasn't set or sandbox altered it
    paths.push(PathBuf::from("/Users/noeljabraham/Library/Android/sdk/platform-tools"));

    if let Ok(android_home) = std::env::var("ANDROID_HOME") {
        paths.push(PathBuf::from(android_home).join("platform-tools"));
    }
    if let Ok(android_sdk) = std::env::var("ANDROID_SDK_ROOT") {
        paths.push(PathBuf::from(android_sdk).join("platform-tools"));
    }
    paths
}

pub fn resolve_tool(tool: &str) -> Option<PathBuf> {
    if !ALLOWED_TOOLS.contains(&tool) {
        return None;
    }
    for dir in tool_search_paths() {
        let candidate = dir.join(tool);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    if let Ok(output) = Command::new("/usr/bin/which").arg(tool).output() {
        if output.status.success() {
            let path_str = String::from_utf8_lossy(&output.stdout).trim().to_owned();
            if !path_str.is_empty() {
                let candidate = PathBuf::from(path_str);
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        }
    }
    None
}

pub fn available(tool: &str) -> bool {
    resolve_tool(tool).is_some()
}

pub fn run<I, S>(tool: &str, args: I) -> Result<Output, String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let tool_path = resolve_tool(tool).ok_or_else(|| {
        if tool == "adb" {
            "ADB is not installed. Run `brew install android-platform-tools` in your terminal to enable Android detection.".to_owned()
        } else {
            format!("Tool '{tool}' is not installed or not found on PATH.")
        }
    })?;

    let path_env = tool_search_paths()
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join(":");

    let mut cmd = Command::new(tool_path);
    cmd.args(args);
    cmd.env_clear();
    cmd.env("PATH", path_env);

    let home = std::env::var("HOME").unwrap_or_else(|_| "/Users/noeljabraham".to_owned());
    cmd.env("HOME", &home);

    for var in &[
        "USER",
        "LOGNAME",
        "TMPDIR",
        "ANDROID_HOME",
        "ANDROID_SDK_ROOT",
        "ADB_SERVER_SOCKET",
    ] {
        if let Ok(val) = std::env::var(var) {
            cmd.env(var, val);
        }
    }

    cmd.output()
        .map_err(|error| format!("Could not run {tool}: {error}"))
}

pub fn checked<I, S>(tool: &str, args: I) -> Result<String, String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let output = run(tool, args)?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        Err(if !stderr.is_empty() {
            stderr
        } else if !stdout.is_empty() {
            stdout
        } else {
            format!("{tool} exited with {}", output.status)
        })
    }
}

pub fn ensure_local_path(path: &Path) -> Result<(), String> {
    if !path.is_absolute() {
        return Err("Use an absolute local path".to_owned());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_unallowed_programs() {
        assert!(run("curl", ["https://example.com"]).is_err());
        assert!(run("sh", ["-c", "ls"]).is_err());
    }

    #[test]
    fn validates_absolute_paths() {
        assert!(ensure_local_path(Path::new("relative/path")).is_err());
        assert!(ensure_local_path(Path::new("/tmp/absolute")).is_ok());
    }
}
