#![allow(dead_code)]

use std::path::PathBuf;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Device {
    pub udid: String,
    pub name: String,
    pub product_type: String,
    pub friendly_model: String,
    pub product_version: String,
    pub serial_number: String,
    pub battery_level: Option<i32>,
    pub battery_state: String,
    pub disk_total_bytes: Option<u64>,
    pub disk_free_bytes: Option<u64>,
    pub connection: String,
}

impl Device {
    pub fn title(&self) -> String {
        if self.name.is_empty() {
            self.udid.clone()
        } else {
            self.name.clone()
        }
    }

    pub fn display_model(&self) -> String {
        if !self.friendly_model.is_empty() {
            self.friendly_model.clone()
        } else if !self.product_type.is_empty() {
            self.product_type.clone()
        } else {
            "Apple Device".to_owned()
        }
    }

    pub fn battery_label(&self) -> String {
        match self.battery_level {
            Some(level) => {
                if !self.battery_state.is_empty() {
                    format!("{level}% ({})", self.battery_state)
                } else {
                    format!("{level}%")
                }
            }
            None => "Unknown".to_owned(),
        }
    }

    pub fn disk_usage_label(&self) -> String {
        match (self.disk_free_bytes, self.disk_total_bytes) {
            (Some(free), Some(total)) => {
                let used = total.saturating_sub(free);
                format!("{:.1} GB used of {:.1} GB", used as f64 / 1e9, total as f64 / 1e9)
            }
            _ => "Not available".to_owned(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AndroidDevice {
    pub serial: String,
    pub model: String,
    pub state: String,
    pub is_ready: bool,
}

impl AndroidDevice {
    pub fn title(&self) -> String {
        if !self.model.is_empty() {
            format!("{} ({})", self.model, self.serial)
        } else {
            self.serial.clone()
        }
    }

    pub fn status_label(&self) -> String {
        match self.state.as_str() {
            "device" => "Ready (USB Debugging Authorized)".to_owned(),
            "unauthorized" => "Unauthorized: Please unlock phone and tap 'Allow USB debugging'".to_owned(),
            "offline" => "Device is offline".to_owned(),
            _ => format!("Status: {}", self.state),
        }
    }
}

#[derive(Clone, Debug)]
pub struct BackupEntry {
    pub path: PathBuf,
    pub device_name: String,
    pub device_udid: String,
    pub modified: String,
    pub size_bytes: u64,
    pub is_encrypted: bool,
}

impl BackupEntry {
    pub fn size_label(&self) -> String {
        let mut size = self.size_bytes as f64;
        let units = ["B", "KB", "MB", "GB", "TB"];
        let mut unit = 0;
        while size >= 1024.0 && unit < units.len() - 1 {
            size /= 1024.0;
            unit += 1;
        }
        format!("{size:.1} {}", units[unit])
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Contact {
    pub id: i64,
    pub first_name: String,
    pub last_name: String,
    pub phone_numbers: Vec<String>,
    pub emails: Vec<String>,
    pub organization: String,
    pub job_title: String,
}

impl Contact {
    pub fn display_name(&self) -> String {
        let full = format!("{} {}", self.first_name, self.last_name).trim().to_owned();
        if !full.is_empty() {
            full
        } else if !self.organization.is_empty() {
            self.organization.clone()
        } else if let Some(phone) = self.phone_numbers.first() {
            phone.clone()
        } else {
            "Unnamed Contact".to_owned()
        }
    }

    pub fn to_vcard(&self) -> String {
        let mut vcard = String::from("BEGIN:VCARD\r\nVERSION:3.0\r\n");
        vcard.push_str(&format!("N:{};{};;;\r\n", self.last_name, self.first_name));
        vcard.push_str(&format!("FN:{}\r\n", self.display_name()));
        if !self.organization.is_empty() {
            vcard.push_str(&format!("ORG:{}\r\n", self.organization));
        }
        if !self.job_title.is_empty() {
            vcard.push_str(&format!("TITLE:{}\r\n", self.job_title));
        }
        for phone in &self.phone_numbers {
            vcard.push_str(&format!("TEL;TYPE=CELL:{phone}\r\n"));
        }
        for email in &self.emails {
            vcard.push_str(&format!("EMAIL;TYPE=INTERNET:{email}\r\n"));
        }
        vcard.push_str("END:VCARD\r\n");
        vcard
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Bookmark {
    pub title: String,
    pub url: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MediaFile {
    pub name: String,
    pub path: PathBuf,
    pub size_bytes: u64,
    pub is_video: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AppInfo {
    pub name: String,
    pub bundle_id: String,
    pub version: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Page {
    #[default]
    Dashboard,
    Manage,
    BackupRestore,
    WhatsApp,
    Toolbox,
    Activity,
    About,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ManageSubTab {
    #[default]
    Photos,
    Contacts,
    Bookmarks,
    Apps,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JobKind {
    RefreshDevices,
    CreateBackup,
    RestoreBackup,
    MountMedia,
    UnmountMedia,
    CopyMedia,
    LoadContacts,
    ExportContacts,
    LoadBookmarks,
    DetectAndroid,
    AcquireWhatsApp,
    InspectWhatsApp,
    BuildWhatsAppArchive,
    PrepareWhatsAppRestore,
    OneClickWhatsAppTransfer,
    RestartDevice,
    ShutdownDevice,
    TakeScreenshot,
    CollectCrashReports,
    DisableBackupEncryption,
}

impl JobKind {
    pub fn label(&self) -> &'static str {
        match self {
            Self::RefreshDevices => "Refresh connected devices (iOS & Android)",
            Self::CreateBackup => "Create backup",
            Self::RestoreBackup => "Restore backup",
            Self::MountMedia => "Mount camera photos",
            Self::UnmountMedia => "Unmount camera photos",
            Self::CopyMedia => "Copy photos/videos",
            Self::LoadContacts => "Read contacts database",
            Self::ExportContacts => "Export vCard contacts",
            Self::LoadBookmarks => "Read Safari bookmarks",
            Self::DetectAndroid => "Detect Android devices",
            Self::AcquireWhatsApp => "Pull WhatsApp database via ADB",
            Self::InspectWhatsApp => "Inspect WhatsApp source",
            Self::BuildWhatsAppArchive => "Normalize WhatsApp messages",
            Self::PrepareWhatsAppRestore => "Prepare iOS WhatsApp restore",
            Self::OneClickWhatsAppTransfer => "Full WhatsApp Transfer (Android ➔ iOS)",
            Self::RestartDevice => "Restart iOS device",
            Self::ShutdownDevice => "Shut down iOS device",
            Self::TakeScreenshot => "Capture device screenshot",
            Self::CollectCrashReports => "Collect device crash reports",
            Self::DisableBackupEncryption => "Disable backup encryption on iPhone",
        }
    }
}

#[derive(Clone, Debug)]
pub enum JobResult {
    Progress {
        percentage: f32,
        message: String,
    },
    DualDevices {
        ios: Vec<Device>,
        android: Vec<AndroidDevice>,
        adb_available: bool,
        android_error: Option<String>,
    },
    Devices(Vec<Device>),
    AndroidDevices(Vec<AndroidDevice>),
    AndroidAcquired(PathBuf),
    ContactsLoaded(Vec<Contact>),
    BookmarksLoaded(Vec<Bookmark>),
    WhatsAppInspected(crate::whatsapp::TransferReport),
    WhatsAppArchived(crate::whatsapp::archive::DatabaseSummary),
    WhatsAppPrepared(crate::whatsapp::ios_backup::PrepareResult),
    Completed(String),
    Failed(String),
}
