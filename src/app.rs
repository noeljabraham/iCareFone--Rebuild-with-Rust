use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

use eframe::egui::{self, Color32, RichText, ScrollArea, Stroke};

use crate::domain::{AndroidDevice, Bookmark, Contact, Device, JobKind, JobResult, ManageSubTab, MediaFile, Page};
use crate::services::{backup, contacts, device, media, toolbox, whatsapp_device};
use crate::whatsapp::{self, TransferReport, archive, ios_backup};

pub struct ICareFoneApp {
    page: Page,
    manage_subtab: ManageSubTab,
    devices: Vec<Device>,
    selected_device: usize,
    android_devices: Vec<AndroidDevice>,
    selected_android: usize,
    adb_available: bool,
    android_error: Option<String>,
    last_device_poll: std::time::Instant,
    device_poll_pending: bool,
    device_sender: Sender<JobResult>,
    device_receiver: Receiver<JobResult>,
    backup_path: String,
    restore_path: String,
    media_mount: String,
    media_destination: String,
    full_backup: bool,
    restore_confirmed: bool,
    media_files: Vec<MediaFile>,
    contacts_db_path: String,
    contacts_list: Vec<Contact>,
    contact_search: String,
    bookmarks_db_path: String,
    bookmarks_list: Vec<Bookmark>,
    whatsapp_source: String,
    whatsapp_key: String,
    whatsapp_workspace: String,
    whatsapp_ios_template: String,
    whatsapp_restore_output: String,
    whatsapp_owner_jid: String,
    whatsapp_report: Option<TransferReport>,
    whatsapp_reuse_existing: bool,
    ios_backup_password: String,
    screenshot_path: String,
    crash_reports_dir: String,
    active_job: Option<JobKind>,
    transfer_progress: Option<(f32, String)>,
    activity: Vec<String>,
    sender: Sender<JobResult>,
    receiver: Receiver<JobResult>,
}

impl ICareFoneApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        cc.egui_ctx.set_visuals(egui::Visuals::dark());
        let (sender, receiver) = mpsc::channel();
        let (device_sender, device_receiver) = mpsc::channel();
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_owned());
        let adb_available = whatsapp_device::is_adb_available();

        let mut app = Self {
            page: Page::Dashboard,
            manage_subtab: ManageSubTab::Photos,
            devices: Vec::new(),
            selected_device: 0,
            android_devices: Vec::new(),
            selected_android: 0,
            adb_available,
            android_error: None,
            last_device_poll: std::time::Instant::now(),
            device_poll_pending: false,
            device_sender,
            device_receiver,
            backup_path: format!("{home}/iCareFone/Backups"),
            restore_path: format!("{home}/iCareFone/Backups"),
            media_mount: format!("{home}/iCareFone/MediaMount"),
            media_destination: format!("{home}/Pictures/iCareFone Import"),
            full_backup: true,
            restore_confirmed: false,
            media_files: Vec::new(),
            contacts_db_path: format!("{home}/iCareFone/AddressBook.sqlitedb"),
            contacts_list: Vec::new(),
            contact_search: String::new(),
            bookmarks_db_path: format!("{home}/iCareFone/Bookmarks.db"),
            bookmarks_list: Vec::new(),
            whatsapp_source: format!("{home}/iCareFone/WhatsApp/Android"),
            whatsapp_key: String::new(),
            whatsapp_workspace: format!("{home}/iCareFone/WhatsApp/Converted"),
            whatsapp_ios_template: format!("{home}/iCareFone/Backups/iOS-template"),
            whatsapp_restore_output: format!("{home}/iCareFone/Backups/iOS-WhatsApp-ready"),
            whatsapp_owner_jid: String::new(),
            whatsapp_report: None,
            whatsapp_reuse_existing: true,
            ios_backup_password: String::new(),
            screenshot_path: format!("{home}/Pictures/iCareFone-Screenshot.png"),
            crash_reports_dir: format!("{home}/iCareFone/CrashReports"),
            active_job: None,
            transfer_progress: None,
            activity: vec!["iCareFone Clone initialized in offline, clean-room mode".to_owned()],
            sender,
            receiver,
        };
        app.refresh_devices();
        app
    }

    fn selected_udid(&self) -> Option<String> {
        self.devices.get(self.selected_device).map(|d| d.udid.clone())
    }

    fn current_device(&self) -> Option<&Device> {
        self.devices.get(self.selected_device)
    }

    fn current_android(&self) -> Option<&AndroidDevice> {
        self.android_devices.get(self.selected_android)
    }

    fn selected_android_serial(&self) -> Option<String> {
        self.android_devices.get(self.selected_android).map(|d| d.serial.clone())
    }

    fn start_job<F>(&mut self, kind: JobKind, task: F)
    where
        F: FnOnce() -> JobResult + Send + 'static,
    {
        self.start_job_with_progress(kind, move |_progress| task());
    }

    fn start_job_with_progress<F>(&mut self, kind: JobKind, task: F)
    where
        F: FnOnce(Sender<JobResult>) -> JobResult + Send + 'static,
    {
        if self.active_job.is_some() {
            return;
        }
        self.activity.push(format!("[{}] Started: {}", current_time_str(), kind.label()));
        self.active_job = Some(kind);
        if kind == JobKind::OneClickWhatsAppTransfer || kind == JobKind::CreateBackup || kind == JobKind::RestoreBackup {
            self.transfer_progress = Some((0.0, format!("Starting {}...", kind.label())));
        }
        let sender = self.sender.clone();
        let progress_sender = self.sender.clone();
        thread::spawn(move || {
            let result = task(progress_sender);
            let _ = sender.send(result);
        });
    }

    fn refresh_devices(&mut self) {
        if self.device_poll_pending {
            return;
        }
        self.device_poll_pending = true;
        let sender = self.device_sender.clone();
        thread::spawn(move || {
            let ios = device::discover().unwrap_or_default();
            let adb_available = whatsapp_device::is_adb_available();
            let (android, android_error) = match whatsapp_device::discover_android() {
                Ok(list) => (list, None),
                Err(err) => (Vec::new(), Some(err)),
            };
            let _ = sender.send(JobResult::DualDevices {
                ios,
                android,
                adb_available,
                android_error,
            });
        });
    }

    fn poll_job(&mut self, ctx: &egui::Context) {
        // 1. Process background device polling independently
        while let Ok(result) = self.device_receiver.try_recv() {
            self.device_poll_pending = false;
            ctx.request_repaint();
            if let JobResult::DualDevices { ios, android, adb_available, android_error } = result {
                self.adb_available = adb_available;
                self.android_error = android_error;

                let prev_ios_len = self.devices.len();
                let prev_android_len = self.android_devices.len();
                let changed = prev_ios_len != ios.len()
                    || prev_android_len != android.len()
                    || self.devices != ios
                    || self.android_devices != android;

                if changed {
                    let mut log_msg = format!(
                        "[{}] Devices updated: {} iOS, {} Android",
                        current_time_str(),
                        ios.len(),
                        android.len()
                    );
                    if !adb_available {
                        log_msg.push_str(" (ADB is not installed; run `brew install android-platform-tools` for Android detection)");
                    }
                    self.activity.push(log_msg);

                    for dev in &android {
                        if dev.state == "unauthorized" {
                            self.activity.push(format!(
                                "[{}] ⚠️ Android device '{}' is unauthorized: Please unlock screen and tap 'Allow USB debugging'.",
                                current_time_str(),
                                dev.serial
                            ));
                        }
                    }
                }

                self.devices = ios;
                self.android_devices = android;
                if self.selected_device >= self.devices.len() {
                    self.selected_device = 0;
                }
                if self.selected_android >= self.android_devices.len() {
                    self.selected_android = 0;
                }
            }
        }

        // 2. Process active user jobs (transfers, backups, restores)
        while let Ok(result) = self.receiver.try_recv() {
            ctx.request_repaint();
            match result {
                JobResult::Progress { percentage, ref message } => {
                    self.transfer_progress = Some((percentage, message.clone()));
                    self.activity.push(format!(
                        "[{}] ⏳ ({:.0}%): {}",
                        current_time_str(),
                        percentage * 100.0,
                        message
                    ));
                    continue;
                }
                _ => {
                    self.active_job = None;
                }
            }
            match result {
                JobResult::Progress { .. } | JobResult::DualDevices { .. } => unreachable!(),
                JobResult::Devices(devices) => {
                    self.activity.push(format!(
                        "[{}] Found {} connected iOS device(s)",
                        current_time_str(),
                        devices.len()
                    ));
                    self.devices = devices;
                    if self.selected_device >= self.devices.len() {
                        self.selected_device = 0;
                    }
                }
                JobResult::AndroidDevices(devices) => {
                    self.activity.push(format!(
                        "[{}] Found {} connected Android device(s)",
                        current_time_str(),
                        devices.len()
                    ));
                    self.android_devices = devices;
                    if self.selected_android >= self.android_devices.len() {
                        self.selected_android = 0;
                    }
                }
                JobResult::AndroidAcquired(path) => {
                    self.whatsapp_source = path.to_string_lossy().to_string();
                    self.activity.push(format!(
                        "[{}] Pulled Android WhatsApp backup to {}",
                        current_time_str(),
                        self.whatsapp_source
                    ));
                }
                JobResult::ContactsLoaded(contacts) => {
                    self.activity.push(format!(
                        "[{}] Loaded {} contacts",
                        current_time_str(),
                        contacts.len()
                    ));
                    self.contacts_list = contacts;
                }
                JobResult::BookmarksLoaded(bookmarks) => {
                    self.activity.push(format!(
                        "[{}] Loaded {} Safari bookmarks",
                        current_time_str(),
                        bookmarks.len()
                    ));
                    self.bookmarks_list = bookmarks;
                }
                JobResult::WhatsAppInspected(report) => {
                    self.activity.push(format!(
                        "[{}] Decrypted WhatsApp database (Format: {}, {} chats, {} messages)",
                        current_time_str(),
                        report.format,
                        report.chats,
                        report.messages
                    ));
                    self.whatsapp_report = Some(report);
                }
                JobResult::WhatsAppArchived(summary) => {
                    self.activity.push(format!(
                        "[{}] Normalized WhatsApp archive created: {} chats, {} messages",
                        current_time_str(),
                        summary.chats,
                        summary.messages
                    ));
                }
                JobResult::WhatsAppPrepared(prep) => {
                    self.activity.push(format!(
                        "[{}] iOS restore package ready at {} ({} chats, {} messages)",
                        current_time_str(),
                        prep.output_backup.display(),
                        prep.conversations_written,
                        prep.messages_written
                    ));
                }
                JobResult::Completed(message) => {
                    self.transfer_progress = Some((1.0, message.clone()));
                    self.activity.push(format!("[{}] Success: {}", current_time_str(), message));
                }
                JobResult::Failed(error) => {
                    self.transfer_progress = None;
                    self.activity.push(format!("[{}] Error: {}", current_time_str(), error));
                }
            }
        }
    }
}

impl eframe::App for ICareFoneApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_job(ctx);

        // Auto-refresh devices every 3 seconds only when idle
        if self.active_job.is_none() && !self.device_poll_pending && self.last_device_poll.elapsed() >= std::time::Duration::from_secs(3) {
            self.last_device_poll = std::time::Instant::now();
            self.refresh_devices();
        }
        ctx.request_repaint_after(std::time::Duration::from_secs(3));

        // Sidebar Panel
        egui::SidePanel::left("navigation_panel")
            .resizable(false)
            .exact_width(230.0)
            .show(ctx, |ui| {
                ui.add_space(14.0);
                ui.heading(RichText::new("📱 iCareFone").size(20.0).strong().color(Color32::from_rgb(0, 168, 255)));
                ui.label(RichText::new("Dual-Device Manager").size(11.0).color(Color32::GRAY));
                ui.add_space(16.0);

                // iOS Device Section
                ui.label(RichText::new("📱 IOS DEVICE").size(10.0).strong().color(Color32::LIGHT_GRAY));
                if self.devices.is_empty() {
                    ui.label(RichText::new("No iOS device detected").size(11.0).italics().color(Color32::DARK_GRAY));
                } else {
                    egui::ComboBox::from_id_salt("device_picker")
                        .width(200.0)
                        .selected_text(self.current_device().map(|d| d.title()).unwrap_or_default())
                        .show_ui(ui, |ui| {
                            for (index, item) in self.devices.iter().enumerate() {
                                ui.selectable_value(&mut self.selected_device, index, item.title());
                            }
                        });
                }

                ui.add_space(8.0);

                // Android Device Section
                ui.label(RichText::new("🤖 ANDROID DEVICE").size(10.0).strong().color(Color32::LIGHT_GRAY));
                if !self.adb_available {
                    ui.label(RichText::new("ADB missing (`brew install android-platform-tools`)").size(10.5).color(Color32::RED));
                } else if self.android_devices.is_empty() {
                    ui.label(RichText::new("No Android device detected").size(11.0).italics().color(Color32::DARK_GRAY));
                } else {
                    egui::ComboBox::from_id_salt("android_sidebar_picker")
                        .width(200.0)
                        .selected_text(self.current_android().map(|d| d.title()).unwrap_or_default())
                        .show_ui(ui, |ui| {
                            for (index, item) in self.android_devices.iter().enumerate() {
                                ui.selectable_value(&mut self.selected_android, index, item.title());
                            }
                        });
                    if let Some(dev) = self.current_android() {
                        let status_color = if dev.is_ready {
                            Color32::GREEN
                        } else if dev.state == "unauthorized" {
                            Color32::YELLOW
                        } else {
                            Color32::RED
                        };
                        ui.label(RichText::new(dev.status_label()).size(10.0).color(status_color));
                    }
                }

                ui.add_space(8.0);
                if ui.button("🔄 Refresh Both Devices").clicked() {
                    self.refresh_devices();
                }

                ui.add_space(14.0);
                ui.separator();
                ui.add_space(8.0);

                // Navigation Tabs
                let pages = [
                    (Page::Dashboard, "📊 Dashboard"),
                    (Page::Manage, "📁 File & Data Manager"),
                    (Page::BackupRestore, "💾 Backup & Restore"),
                    (Page::WhatsApp, "💬 WhatsApp Transfer"),
                    (Page::Toolbox, "🛠️ More Tools"),
                    (Page::Activity, "📜 Activity Log"),
                    (Page::About, "ℹ️ About"),
                ];

                for (page, label) in pages {
                    let is_active = self.page == page;
                    let text = RichText::new(label).size(13.5);
                    let button = egui::Button::new(if is_active {
                        text.strong().color(Color32::WHITE)
                    } else {
                        text.color(Color32::LIGHT_GRAY)
                    })
                    .fill(if is_active { Color32::from_rgb(0, 122, 255) } else { Color32::TRANSPARENT })
                    .min_size(egui::vec2(200.0, 32.0));

                    if ui.add(button).clicked() {
                        self.page = page;
                    }
                    ui.add_space(2.0);
                }

                ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
                    ui.add_space(10.0);
                    if let Some((pct, msg)) = &self.transfer_progress {
                        let color = if *pct >= 1.0 { Color32::GREEN } else { Color32::from_rgb(0, 168, 255) };
                        ui.label(RichText::new(format!("{:.0}% - {}", pct * 100.0, msg)).size(10.5).color(color));
                        let bar = egui::ProgressBar::new(*pct).show_percentage();
                        ui.add(bar);
                    } else if let Some(job) = &self.active_job {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label(RichText::new(job.label()).size(11.0).color(Color32::YELLOW));
                        });
                    } else {
                        ui.label(RichText::new("● Idle / Ready").size(11.0).color(Color32::GREEN));
                    }
                    ui.separator();
                });
            });

        // Central Content Area
        egui::CentralPanel::default().show(ctx, |ui| {
            match self.page {
                Page::Dashboard => self.render_dashboard(ui),
                Page::Manage => self.render_manage(ui),
                Page::BackupRestore => self.render_backup_restore(ui),
                Page::WhatsApp => self.render_whatsapp(ui),
                Page::Toolbox => self.render_toolbox(ui),
                Page::Activity => self.render_activity(ui),
                Page::About => self.render_about(ui),
            }
        });
    }
}

impl ICareFoneApp {
    fn render_dashboard(&mut self, ui: &mut egui::Ui) {
        ui.heading("Device Overview & Dashboard");
        ui.label("Real-time inspection of your connected iPhone/iPad and Android devices.");
        ui.add_space(14.0);

        // Dual Device Status Banner
        let has_ios = !self.devices.is_empty();
        let has_android = !self.android_devices.is_empty();
        if has_ios && has_android {
            egui::Frame::group(ui.style())
                .fill(Color32::from_rgb(20, 45, 20))
                .stroke(Stroke::new(1.0_f32, Color32::from_rgb(40, 90, 40)))
                .inner_margin(12.0)
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("✅ Dual Connection Ready:").strong().color(Color32::GREEN));
                        ui.label("Both Android and iOS devices are connected! You can transfer WhatsApp data directly.");
                        if ui.button("🚀 Transfer WhatsApp Now").clicked() {
                            self.page = Page::WhatsApp;
                        }
                    });
                });
            ui.add_space(12.0);
        }

        if let Some(device) = self.current_device().cloned() {
            // Hero Card for iOS
            egui::Frame::group(ui.style())
                .fill(Color32::from_rgb(28, 28, 30))
                .stroke(Stroke::new(1.0_f32, Color32::from_rgb(44, 44, 46)))
                .inner_margin(16.0)
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("📱").size(48.0));
                        ui.add_space(10.0);
                        ui.vertical(|ui| {
                            ui.heading(RichText::new(&device.name).size(18.0).strong());
                            ui.label(RichText::new(device.display_model()).size(14.0).color(Color32::LIGHT_GRAY));
                            ui.label(format!("iOS Version: {}", device.product_version));
                            ui.label(format!("Serial Number: {}", device.serial_number));
                            ui.label(RichText::new(format!("UDID: {}", device.udid)).size(10.5).color(Color32::DARK_GRAY));
                        });
                    });

                    ui.add_space(14.0);
                    ui.separator();
                    ui.add_space(10.0);

                    // Stats row
                    ui.columns(3, |cols| {
                        cols[0].vertical(|ui| {
                            ui.label(RichText::new("Battery Status").strong());
                            ui.label(device.battery_label());
                        });
                        cols[1].vertical(|ui| {
                            ui.label(RichText::new("Storage Space").strong());
                            ui.label(device.disk_usage_label());
                        });
                        cols[2].vertical(|ui| {
                            ui.label(RichText::new("Connection Type").strong());
                            ui.label(&device.connection);
                        });
                    });
                });
        } else {
            ui.vertical_centered(|ui| {
                ui.add_space(20.0);
                ui.label(RichText::new("🔌 No iOS Device Connected").size(20.0).strong());
                ui.label("Connect your iPhone or iPad using a USB cable and tap 'Trust This Computer'.");
            });
        }

        ui.add_space(16.0);

        // Android Card on Dashboard
        if let Some(android) = self.current_android() {
            egui::Frame::group(ui.style())
                .fill(Color32::from_rgb(28, 30, 28))
                .stroke(Stroke::new(1.0_f32, Color32::from_rgb(44, 50, 44)))
                .inner_margin(14.0)
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("🤖").size(40.0));
                        ui.add_space(10.0);
                        ui.vertical(|ui| {
                            ui.heading(RichText::new(&android.model).size(16.0).strong());
                            ui.label(format!("Serial Number: {}", android.serial));
                            let color = if android.is_ready { Color32::GREEN } else { Color32::YELLOW };
                            ui.label(RichText::new(android.status_label()).color(color));
                        });
                    });
                });
        }

        ui.add_space(20.0);
        ui.heading("Quick Actions");
        ui.add_space(8.0);

        ui.columns(3, |cols| {
            cols[0].group(|ui| {
                ui.heading("💾 Backup");
                ui.label("Create a safe local backup of your device.");
                if ui.button("Go to Backup").clicked() {
                    self.page = Page::BackupRestore;
                }
            });
            cols[1].group(|ui| {
                ui.heading("🖼️ Export Photos");
                ui.label("Import/export camera roll photos and videos.");
                if ui.button("Manage Photos").clicked() {
                    self.page = Page::Manage;
                    self.manage_subtab = ManageSubTab::Photos;
                }
            });
            cols[2].group(|ui| {
                ui.heading("💬 WhatsApp");
                ui.label("Migrate WhatsApp messages from Android.");
                if ui.button("Start Transfer").clicked() {
                    self.page = Page::WhatsApp;
                }
            });
        });
    }

    fn render_manage(&mut self, ui: &mut egui::Ui) {
        ui.heading("File & Data Manager");
        ui.label("Browse, export, and manage data from your iOS device.");
        ui.add_space(10.0);

        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.manage_subtab, ManageSubTab::Photos, "🖼️ Photos & Videos");
            ui.selectable_value(&mut self.manage_subtab, ManageSubTab::Contacts, "👤 Contacts");
            ui.selectable_value(&mut self.manage_subtab, ManageSubTab::Bookmarks, "🔖 Safari Bookmarks");
        });
        ui.separator();
        ui.add_space(10.0);

        match self.manage_subtab {
            ManageSubTab::Photos => self.render_manage_photos(ui),
            ManageSubTab::Contacts => self.render_manage_contacts(ui),
            ManageSubTab::Bookmarks => self.render_manage_bookmarks(ui),
            ManageSubTab::Apps => {},
        }
    }

    fn render_manage_photos(&mut self, ui: &mut egui::Ui) {
        ui.label(RichText::new("Camera Roll & Photos").size(16.0).strong());
        ui.label("Mount the device's DCIM media tree locally using `ifuse` to browse and batch export media.");
        ui.add_space(8.0);

        ui.horizontal(|ui| {
            ui.label("Mount Path:");
            ui.text_edit_singleline(&mut self.media_mount);
        });
        ui.horizontal(|ui| {
            ui.label("Export Target:");
            ui.text_edit_singleline(&mut self.media_destination);
        });

        ui.add_space(8.0);
        ui.horizontal(|ui| {
            let udid = self.selected_udid();
            let can_mount = udid.is_some() && self.active_job.is_none();
            if ui.add_enabled(can_mount, egui::Button::new("📷 Mount Camera Media")).clicked() {
                let point = PathBuf::from(&self.media_mount);
                let target_udid = udid.unwrap();
                self.start_job(JobKind::MountMedia, move || match media::mount(&point, &target_udid) {
                    Ok(msg) => JobResult::Completed(msg),
                    Err(err) => JobResult::Failed(err),
                });
            }

            if ui.button("⏹ Unmount Media").clicked() {
                let point = PathBuf::from(&self.media_mount);
                self.start_job(JobKind::UnmountMedia, move || match media::unmount(&point) {
                    Ok(msg) => JobResult::Completed(msg),
                    Err(err) => JobResult::Failed(err),
                });
            }

            if ui.button("📂 Refresh Media List").clicked() {
                let point = PathBuf::from(&self.media_mount);
                self.media_files = media::list_media(&point);
            }

            if ui.button("⬇ Export All Media").clicked() {
                let from = PathBuf::from(&self.media_mount);
                let to = PathBuf::from(&self.media_destination);
                self.start_job(JobKind::CopyMedia, move || match media::copy_tree(&from, &to) {
                    Ok(msg) => JobResult::Completed(msg),
                    Err(err) => JobResult::Failed(err),
                });
            }
        });

        ui.add_space(14.0);
        ui.label(format!("Discovered {} photo/video items:", self.media_files.len()));
        ScrollArea::vertical().max_height(350.0).show(ui, |ui| {
            for file in &self.media_files {
                ui.horizontal(|ui| {
                    ui.label(if file.is_video { "🎥" } else { "🖼️" });
                    ui.label(&file.name);
                    ui.label(RichText::new(format!("{:.1} MB", file.size_bytes as f64 / 1e6)).color(Color32::DARK_GRAY));
                });
            }
        });
    }

    fn render_manage_contacts(&mut self, ui: &mut egui::Ui) {
        ui.label(RichText::new("Contacts Manager").size(16.0).strong());
        ui.label("Directly read and export contacts from an AddressBook.sqlitedb database.");
        ui.add_space(8.0);

        ui.horizontal(|ui| {
            ui.label("Database Path:");
            ui.text_edit_singleline(&mut self.contacts_db_path);
            if ui.button("📖 Read Contacts").clicked() {
                let path = PathBuf::from(&self.contacts_db_path);
                self.start_job(JobKind::LoadContacts, move || match contacts::parse_addressbook(&path) {
                    Ok(list) => JobResult::ContactsLoaded(list),
                    Err(err) => JobResult::Failed(err),
                });
            }
        });

        ui.horizontal(|ui| {
            ui.label("Search:");
            ui.text_edit_singleline(&mut self.contact_search);
            if ui.button("Export to vCard (.vcf)").clicked() {
                let home = std::env::var("HOME").unwrap_or_default();
                let dest = PathBuf::from(format!("{home}/Contacts-Export.vcf"));
                let list = self.contacts_list.clone();
                self.start_job(JobKind::ExportContacts, move || match contacts::export_vcards(&list, &dest) {
                    Ok(msg) => JobResult::Completed(msg),
                    Err(err) => JobResult::Failed(err),
                });
            }
        });

        ui.add_space(10.0);
        ScrollArea::vertical().max_height(350.0).show(ui, |ui| {
            for contact in &self.contacts_list {
                if !self.contact_search.is_empty()
                    && !contact.display_name().to_lowercase().contains(&self.contact_search.to_lowercase())
                {
                    continue;
                }
                ui.group(|ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(contact.display_name()).strong());
                        if !contact.organization.is_empty() {
                            ui.label(format!("({})", contact.organization));
                        }
                    });
                    if !contact.phone_numbers.is_empty() {
                        ui.label(format!("📞 {}", contact.phone_numbers.join(", ")));
                    }
                    if !contact.emails.is_empty() {
                        ui.label(format!("✉️ {}", contact.emails.join(", ")));
                    }
                });
            }
        });
    }

    fn render_manage_bookmarks(&mut self, ui: &mut egui::Ui) {
        ui.label(RichText::new("Safari Bookmarks").size(16.0).strong());
        ui.label("Inspect bookmarks stored in Safari's Bookmarks.db SQLite database.");
        ui.add_space(8.0);

        ui.horizontal(|ui| {
            ui.label("Database Path:");
            ui.text_edit_singleline(&mut self.bookmarks_db_path);
            if ui.button("📖 Read Bookmarks").clicked() {
                let path = PathBuf::from(&self.bookmarks_db_path);
                self.start_job(JobKind::LoadBookmarks, move || match toolbox::read_bookmarks(&path) {
                    Ok(list) => JobResult::BookmarksLoaded(list),
                    Err(err) => JobResult::Failed(err),
                });
            }
        });

        ui.add_space(10.0);
        ScrollArea::vertical().max_height(350.0).show(ui, |ui| {
            for bookmark in &self.bookmarks_list {
                ui.horizontal(|ui| {
                    ui.label("🔖");
                    ui.label(RichText::new(&bookmark.title).strong());
                    ui.hyperlink(&bookmark.url);
                });
            }
        });
    }

    fn render_backup_restore(&mut self, ui: &mut egui::Ui) {
        ui.heading("Backup & Restore");
        ui.label("Create safe, offline iOS backups or restore your device cleanly.");
        ui.add_space(12.0);

        ui.columns(2, |cols| {
            cols[0].group(|ui| {
                ui.heading("💾 Create Backup");
                ui.label("Destination Folder:");
                ui.text_edit_singleline(&mut self.backup_path);
                ui.checkbox(&mut self.full_backup, "Full backup (includes all domains)");

                let can_backup = self.selected_udid().is_some() && self.active_job.is_none();
                if ui.add_enabled(can_backup, egui::Button::new("Start Backup Now")).clicked() {
                    let dest = PathBuf::from(&self.backup_path);
                    let udid = self.selected_udid().unwrap();
                    let full = self.full_backup;
                    self.start_job(JobKind::CreateBackup, move || match backup::create(&dest, &udid, full) {
                        Ok(msg) => JobResult::Completed(msg),
                        Err(err) => JobResult::Failed(err),
                    });
                }
            });

            cols[1].group(|ui| {
                ui.heading("🔄 Restore Backup");
                ui.label("Backup Folder to Restore:");
                ui.text_edit_singleline(&mut self.restore_path);
                ui.checkbox(&mut self.restore_confirmed, "I understand this will overwrite current device data");

                let can_restore = self.selected_udid().is_some() && self.restore_confirmed && self.active_job.is_none();
                if ui.add_enabled(can_restore, egui::Button::new("Restore to Device")).clicked() {
                    let src = PathBuf::from(&self.restore_path);
                    let udid = self.selected_udid().unwrap();
                    self.start_job(JobKind::RestoreBackup, move || match backup::restore(&src, &udid) {
                        Ok(msg) => JobResult::Completed(msg),
                        Err(err) => JobResult::Failed(err),
                    });
                }
            });
        });

        ui.add_space(16.0);
        ui.heading("Saved Backups");
        let backups = backup::list(Path::new(&self.backup_path));
        if backups.is_empty() {
            ui.label(RichText::new("No backups found in current backup path.").italics().color(Color32::DARK_GRAY));
        } else {
            ScrollArea::vertical().max_height(240.0).show(ui, |ui| {
                for b in backups {
                    ui.group(|ui| {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(&b.device_name).strong());
                            ui.label(format!("UDID: {}", b.device_udid));
                            ui.label(format!("Size: {}", b.size_label()));
                            ui.label(format!("Date: {}", b.modified));
                            if b.is_encrypted {
                                ui.label(RichText::new("🔒 Encrypted").color(Color32::YELLOW));
                            }
                        });
                        ui.label(RichText::new(b.path.display().to_string()).size(11.0).color(Color32::DARK_GRAY));
                    });
                }
            });
        }
    }

    fn render_whatsapp(&mut self, ui: &mut egui::Ui) {
        ui.heading("WhatsApp Transfer (Android ➔ iOS)");
        ui.label("Simultaneous dual-device transfer from Android directly into your iPhone.");
        ui.add_space(12.0);

        // Live Percentage & Step Progress Indicator Card
        let mut dismiss_progress = false;
        if let Some((pct, ref msg)) = self.transfer_progress {
            let is_finished = pct >= 1.0;
            egui::Frame::group(ui.style())
                .fill(if is_finished { Color32::from_rgb(18, 42, 25) } else { Color32::from_rgb(18, 32, 52) })
                .stroke(Stroke::new(1.5_f32, if is_finished { Color32::GREEN } else { Color32::from_rgb(0, 168, 255) }))
                .inner_margin(14.0)
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        let icon = if is_finished { "🎉" } else { "⏳" };
                        let title = if is_finished { "WhatsApp Transfer Completed!" } else { "WhatsApp Transfer In Progress" };
                        ui.label(RichText::new(format!("{icon} {title}")).strong().size(15.5).color(if is_finished { Color32::GREEN } else { Color32::from_rgb(0, 168, 255) }));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if is_finished {
                                if ui.button("Dismiss").clicked() {
                                    dismiss_progress = true;
                                }
                            }
                            ui.label(RichText::new(format!("{:.0}%", pct * 100.0)).strong().size(18.0).color(Color32::WHITE));
                        });
                    });

                    ui.add_space(8.0);
                    let bar = egui::ProgressBar::new(pct)
                        .show_percentage()
                        .animate(!is_finished);
                    ui.add(bar);

                    ui.add_space(6.0);
                    ui.label(RichText::new(msg).size(12.5).color(Color32::LIGHT_GRAY));

                    ui.add_space(10.0);
                    ui.separator();
                    ui.add_space(6.0);

                    // 6-Step Visual Breakdown
                    let steps = [
                        ("Step 1", "Extract from Android", 0.05_f32, 0.25_f32),
                        ("Step 2", "Decrypt database", 0.25_f32, 0.45_f32),
                        ("Step 3", "Normalize SQLite archive", 0.45_f32, 0.65_f32),
                        ("Step 4", "Prepare iOS template", 0.65_f32, 0.82_f32),
                        ("Step 5", "Inject into iOS package", 0.82_f32, 0.92_f32),
                        ("Step 6", "Restore & reboot iPhone", 0.92_f32, 1.0_f32),
                    ];

                    ui.columns(2, |step_cols| {
                        for (idx, (num, name, start_pct, end_pct)) in steps.iter().enumerate() {
                            let col = &mut step_cols[idx % 2];
                            let (badge, color) = if pct >= *end_pct || is_finished {
                                ("✅", Color32::GREEN)
                            } else if pct >= *start_pct {
                                ("▶", Color32::YELLOW)
                            } else {
                                ("○", Color32::DARK_GRAY)
                            };
                            col.horizontal(|ui| {
                                ui.label(RichText::new(badge).color(color).strong());
                                ui.label(RichText::new(format!("{num}: {name}")).size(11.5).color(color));
                            });
                        }
                    });
                });
            ui.add_space(12.0);
        }
        if dismiss_progress {
            self.transfer_progress = None;
        }

        // ADB Missing Warning Banner
        if !self.adb_available {
            egui::Frame::group(ui.style())
                .fill(Color32::from_rgb(50, 20, 20))
                .stroke(Stroke::new(1.0_f32, Color32::RED))
                .inner_margin(12.0)
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("⚠️ ADB Missing:").strong().color(Color32::RED));
                        ui.label("Android Debug Bridge is required. Run `brew install android-platform-tools` in your terminal, then click 'Refresh Both Devices'.");
                    });
                });
            ui.add_space(10.0);
        }

        // Dual Device Side-by-Side Display
        ui.columns(2, |cols| {
            // Source Android Box
            cols[0].group(|ui| {
                ui.heading("🤖 Source: Android Device");
                ui.add_space(4.0);

                if !self.adb_available {
                    ui.label(RichText::new("⚠️ ADB Not Installed").strong().color(Color32::RED));
                    ui.label("To detect Android devices, install ADB by running in your terminal:");
                    ui.code("brew install android-platform-tools");
                    ui.add_space(4.0);
                    if ui.button("🔄 Check ADB Again").clicked() {
                        self.refresh_devices();
                    }
                } else if self.android_devices.is_empty() {
                    ui.label(RichText::new("No Android device detected").italics().color(Color32::DARK_GRAY));
                    if let Some(err) = &self.android_error {
                        ui.label(RichText::new(format!("ADB notice: {err}")).size(11.0).color(Color32::YELLOW));
                    }
                    ui.add_space(4.0);
                    ui.label(RichText::new("How to connect your Android device:").strong().size(12.0));
                    ui.label("1. Connect your Android phone with a data USB cable.");
                    ui.label("2. Open Settings > About phone, tap 'Build number' 7 times.");
                    ui.label("3. Open Settings > System / Developer options, turn ON 'USB debugging'.");
                    ui.label("4. Unlock phone screen and tap 'Always allow from this computer'.");
                    ui.label("5. Ensure USB connection is set to 'File Transfer (MTP)'.");
                    ui.add_space(4.0);
                    if ui.button("🔄 Check Connection Now").clicked() {
                        self.refresh_devices();
                    }
                } else {
                    egui::ComboBox::from_id_salt("android_source_picker")
                        .width(260.0)
                        .selected_text(self.current_android().map(|d| d.title()).unwrap_or_default())
                        .show_ui(ui, |ui| {
                            for (idx, dev) in self.android_devices.iter().enumerate() {
                                ui.selectable_value(&mut self.selected_android, idx, dev.title());
                            }
                        });

                    if let Some(dev) = self.current_android() {
                        if dev.is_ready {
                            ui.label(RichText::new("✅ Ready (USB Debugging Authorized)").color(Color32::GREEN).strong());
                            ui.label(RichText::new(format!("Serial: {}", dev.serial)).size(11.0).color(Color32::DARK_GRAY));
                        } else if dev.state == "unauthorized" {
                            ui.group(|ui| {
                                ui.label(RichText::new("⚠️ Authorization Required").strong().color(Color32::YELLOW));
                                ui.label("Please unlock your phone screen and tap 'Allow USB debugging'. Check 'Always allow from this computer'.");
                                if ui.button("🔄 Re-check Authorization").clicked() {
                                    self.refresh_devices();
                                }
                            });
                        } else {
                            ui.label(RichText::new(dev.status_label()).color(Color32::YELLOW));
                        }
                    }
                }
            });

            // Destination iOS Box
            cols[1].group(|ui| {
                ui.heading("📱 Destination: iOS Device");
                ui.add_space(4.0);
                if self.devices.is_empty() {
                    ui.label(RichText::new("No iOS device detected").italics().color(Color32::DARK_GRAY));
                    ui.add_space(4.0);
                    ui.label(RichText::new("How to connect your iPhone:").strong().size(12.0));
                    ui.label("1. Connect iPhone with Lightning / USB-C cable.");
                    ui.label("2. Unlock the screen and tap 'Trust This Computer'.");
                    ui.label("3. Enter passcode on the iPhone if prompted.");
                    ui.add_space(4.0);
                    if ui.button("🔄 Check Connection Now").clicked() {
                        self.refresh_devices();
                    }
                } else {
                    egui::ComboBox::from_id_salt("ios_dest_picker")
                        .width(260.0)
                        .selected_text(self.current_device().map(|d| d.title()).unwrap_or_default())
                        .show_ui(ui, |ui| {
                            for (idx, dev) in self.devices.iter().enumerate() {
                                ui.selectable_value(&mut self.selected_device, idx, dev.title());
                            }
                        });
                    if let Some(dev) = self.current_device() {
                        ui.label(RichText::new(format!("Model: {} (iOS {})", dev.display_model(), dev.product_version)).color(Color32::GREEN).strong());
                        ui.label(RichText::new(format!("UDID: {}", dev.udid)).size(11.0).color(Color32::DARK_GRAY));
                    }
                }
            });
        });

        ui.add_space(14.0);

        // One-Click Transfer Hero Button
        let android_ready = self.current_android().map(|d| d.is_ready).unwrap_or(false);
        let ios_ready = self.current_device().is_some();
        let can_one_click = android_ready && ios_ready && self.active_job.is_none();

        egui::Frame::group(ui.style())
            .fill(if can_one_click { Color32::from_rgb(18, 36, 60) } else { Color32::from_rgb(26, 26, 28) })
            .stroke(Stroke::new(1.0_f32, if can_one_click { Color32::from_rgb(0, 122, 255) } else { Color32::from_rgb(44, 44, 46) }))
            .inner_margin(16.0)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.heading(RichText::new("⚡ One-Click Full Transfer").size(16.0).strong());
                        ui.label("Automatically pulls WhatsApp from Android, normalizes the database, and merges it into your iPhone.");
                        ui.label(RichText::new("🔒 Safe Transfer: Only WhatsApp is updated. Your iPhone is NOT erased or factory reset.").size(11.5).color(Color32::from_rgb(100, 220, 140)));

                        let has_local_archive = PathBuf::from(&self.whatsapp_workspace).join("icarefone-whatsapp.sqlite").is_file();
                        if has_local_archive {
                            ui.add_space(4.0);
                            ui.checkbox(
                                &mut self.whatsapp_reuse_existing,
                                RichText::new("⚡ Quick Resume: Reuse already-extracted WhatsApp database on this Mac (skip 10-min USB pull)").color(Color32::from_rgb(0, 190, 255)).strong(),
                            );
                        }
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let btn = egui::Button::new(RichText::new("🚀 Start Transfer").size(15.0).strong())
                            .min_size(egui::vec2(150.0, 38.0));
                        if ui.add_enabled(can_one_click, btn).clicked() {
                            let serial = self.selected_android_serial().unwrap();
                            let ios_udid = self.selected_udid().unwrap();
                            let source_dir = PathBuf::from(&self.whatsapp_source);
                            let ws = PathBuf::from(&self.whatsapp_workspace);
                            let key = if self.whatsapp_key.is_empty() { None } else { Some(self.whatsapp_key.clone()) };
                            let tmpl = PathBuf::from(&self.whatsapp_ios_template);
                            let out = PathBuf::from(&self.whatsapp_restore_output);
                            let reuse_extracted = self.whatsapp_reuse_existing;
                            let detected_jid = ios_backup::detect_owner_jid(&tmpl)
                                .or_else(|| ios_backup::detect_owner_jid(&tmpl.join(&ios_udid)));
                            let jid = if !self.whatsapp_owner_jid.is_empty() {
                                self.whatsapp_owner_jid.clone()
                            } else if let Some(det) = detected_jid {
                                det
                            } else {
                                "971509592603@s.whatsapp.net".to_owned()
                            };

                            self.start_job_with_progress(JobKind::OneClickWhatsAppTransfer, move |progress| {
                                let archive_path = archive::archive_path(&ws);
                                let has_valid_archive = if archive_path.is_file() {
                                    if let Ok(conn) = rusqlite::Connection::open_with_flags(&archive_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY) {
                                        conn.query_row("SELECT COUNT(*) FROM conversation", [], |r| r.get::<_, i64>(0)).unwrap_or(0) > 0
                                    } else {
                                        false
                                    }
                                } else {
                                    false
                                };

                                if reuse_extracted && has_valid_archive {
                                    let _ = progress.send(JobResult::Progress {
                                        percentage: 0.50,
                                        message: "⚡ Quick Resume: Using already-extracted & converted WhatsApp archive on Mac (skipped ADB pull)...".to_owned(),
                                    });
                                } else {
                                    let _ = progress.send(JobResult::Progress {
                                        percentage: 0.10,
                                        message: "Step 1/6: Pulling WhatsApp database & media from Android via ADB...".to_owned(),
                                    });
                                    // 1. Pull backup from Android
                                    let pull_res = whatsapp_device::acquire_public_backup(&serial, &source_dir);
                                    if let Err(e) = pull_res {
                                        return JobResult::Failed(format!("Step 1/6 (Pull from Android) Failed: {e}"));
                                    }

                                    let _ = progress.send(JobResult::Progress {
                                        percentage: 0.30,
                                        message: "Step 2/6: Decrypting WhatsApp database (AES-256-GCM)...".to_owned(),
                                    });
                                    // 2. Decrypt WhatsApp database
                                    let dec_res = whatsapp::inspect_and_decrypt(&source_dir, key.as_deref(), &ws);
                                    let report = match dec_res {
                                        Ok(r) => r,
                                        Err(e) => return JobResult::Failed(format!("Step 2/6 (Decrypt WhatsApp) Failed: {e}")),
                                    };

                                    let _ = progress.send(JobResult::Progress {
                                        percentage: 0.50,
                                        message: format!("Step 3/6: Normalizing {} chats & {} messages into SQLite archive...", report.chats, report.messages),
                                    });
                                    // 3. Build Archive
                                    if let Err(e) = archive::build_normalized_archive(&report.decrypted_database, report.media_root.as_deref(), &archive_path) {
                                        return JobResult::Failed(format!("Step 3/6 (Normalize Archive) Failed: {e}"));
                                    }
                                }

                                let _ = progress.send(JobResult::Progress {
                                    percentage: 0.65,
                                    message: "Step 4/6: Verifying iOS baseline template backup...".to_owned(),
                                });
                                // 4. Check & prepare iOS baseline template
                                let candidate_tmpl = if ios_backup::is_valid_unencrypted_backup(&tmpl) {
                                    Some(tmpl.clone())
                                } else if ios_backup::is_valid_unencrypted_backup(&tmpl.join(&ios_udid)) {
                                    Some(tmpl.join(&ios_udid))
                                } else {
                                    None
                                };

                                let effective_tmpl = if let Some(valid_tmpl) = candidate_tmpl {
                                    valid_tmpl
                                } else {
                                    // Remove stale or encrypted template backup before creating fresh one
                                    if tmpl.join(&ios_udid).exists() {
                                        let _ = std::fs::remove_dir_all(tmpl.join(&ios_udid));
                                    }

                                    let _ = progress.send(JobResult::Progress {
                                        percentage: 0.70,
                                        message: "Step 4/6: Creating baseline backup of iPhone via idevicebackup2...".to_owned(),
                                    });
                                    if let Err(e) = backup::create(&tmpl, &ios_udid, false) {
                                        return JobResult::Failed(format!("Step 4/6 (Create iOS baseline backup) Failed: {e}"));
                                    }
                                    let new_tmpl = if tmpl.join(&ios_udid).join("Manifest.db").is_file() {
                                        tmpl.join(&ios_udid)
                                    } else {
                                        tmpl.clone()
                                    };

                                    if !ios_backup::is_valid_unencrypted_backup(&new_tmpl) {
                                        return JobResult::Failed(
                                            "Step 4/6 Failed: The iPhone created an ENCRYPTED backup because 'Encrypt local backup' is turned ON in macOS Finder.\n\n\
                                            Apple encrypts all databases in this mode, which prevents offline WhatsApp database injection.\n\n\
                                            To allow WhatsApp transfer to succeed:\n\
                                            1. Open macOS Finder and click your iPhone in the left sidebar.\n\
                                            2. Under the 'General' tab in the 'Backups' section, UNCHECK 'Encrypt local backup'.\n\
                                            3. Enter your iPhone backup password when prompted by macOS.\n\
                                            4. Return here and click 'Start Transfer' again!\n\n\
                                            (Alternatively, enter your backup password in Advanced Transfer Options below and click 'Turn Off Backup Encryption').".to_owned()
                                        );
                                    }
                                    new_tmpl
                                };

                                let _ = progress.send(JobResult::Progress {
                                    percentage: 0.82,
                                    message: "Step 5/6: Injecting ChatStorage.sqlite & media into iOS backup package...".to_owned(),
                                });
                                // 5. Prepare iOS restore package
                                let prep = match ios_backup::prepare_restore(&archive_path, &effective_tmpl, &out, &jid, &ios_udid) {
                                    Ok(prep) => prep,
                                    Err(e) => return JobResult::Failed(format!("Step 5/6 (Inject into iOS backup) Failed: {e}")),
                                };

                                let _ = progress.send(JobResult::Progress {
                                    percentage: 0.92,
                                    message: "Step 6/6: Restoring WhatsApp package to iPhone via idevicebackup2 (device will restart)...".to_owned(),
                                });
                                // 6. Restore package directly to iPhone!
                                match backup::restore(&prep.output_backup, &ios_udid) {
                                    Ok(_) => JobResult::Completed(format!(
                                        "WhatsApp transfer to iPhone completed successfully! Restored {} chats and {} messages. Open WhatsApp on your iPhone to verify.",
                                        prep.conversations_written,
                                        prep.messages_written
                                    )),
                                    Err(e) => JobResult::Failed(format!("Step 6/6 (Restore to iPhone) Failed: {e}")),
                                }
                            });
                        }
                    });
                });
            });

        ui.add_space(14.0);
        ui.separator();
        ui.add_space(8.0);

        ui.label(RichText::new("⚙️ Advanced Transfer Options").size(14.0).strong());
        ui.horizontal(|ui| {
            ui.label("Account JID:");
            ui.text_edit_singleline(&mut self.whatsapp_owner_jid);
            ui.label(RichText::new("(e.g. 15551234567@s.whatsapp.net)").color(Color32::DARK_GRAY));
        });
        ui.horizontal(|ui| {
            ui.label("Crypt Key File (Optional for crypt12/14/15):");
            ui.text_edit_singleline(&mut self.whatsapp_key);
        });
        ui.horizontal(|ui| {
            ui.label("iOS Backup Template:");
            ui.text_edit_singleline(&mut self.whatsapp_ios_template);
        });
        ui.horizontal(|ui| {
            ui.label("Prepared Output Backup:");
            ui.text_edit_singleline(&mut self.whatsapp_restore_output);
        });

        ui.add_space(8.0);
        ui.group(|ui| {
            ui.label(RichText::new("🔒 iPhone Backup Encryption Manager").strong());
            ui.label("WhatsApp transfer requires an unencrypted baseline backup so SQLite databases can be merged offline.");
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label("iPhone Backup Password:");
                ui.add(egui::TextEdit::singleline(&mut self.ios_backup_password).password(true));
                let can_disable = !self.ios_backup_password.is_empty() && self.selected_udid().is_some() && self.active_job.is_none();
                if ui.add_enabled(can_disable, egui::Button::new("🔓 Turn Off Backup Encryption")).clicked() {
                    let u = self.selected_udid().unwrap();
                    let pw = self.ios_backup_password.clone();
                    let tmpl_path = PathBuf::from(&self.whatsapp_ios_template);
                    self.start_job(JobKind::DisableBackupEncryption, move || {
                        match backup::disable_encryption(&u, &pw, &tmpl_path) {
                            Ok(msg) => JobResult::Completed(format!("Backup encryption turned off: {msg}. You can now run Start Transfer!")),
                            Err(err) => JobResult::Failed(format!("Failed to disable encryption: {err}. Please uncheck 'Encrypt local backup' in macOS Finder.")),
                        }
                    });
                }
            });
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                if ui.button("🗑️ Clear Cached Template Backup").clicked() {
                    let tmpl_path = PathBuf::from(&self.whatsapp_ios_template);
                    let _ = backup::clean_template(&tmpl_path);
                    self.activity.push(format!("[{}] Cleared cached iOS template backup", current_time_str()));
                }
                ui.label(RichText::new("Clears cached backup so a fresh unencrypted backup will be taken on next transfer").size(11.0).color(Color32::DARK_GRAY));
            });
        });

        if let Some(rep) = &self.whatsapp_report {
            ui.add_space(8.0);
            ui.label(RichText::new(format!("📊 Current Archive: {} chats, {} messages (Format: {})", rep.chats, rep.messages, rep.format)).color(Color32::LIGHT_BLUE));
        }
    }

    fn render_toolbox(&mut self, ui: &mut egui::Ui) {
        ui.heading("More Tools & Utilities");
        ui.label("System operations and diagnostics directly via libimobiledevice.");
        ui.add_space(14.0);

        let udid = self.selected_udid();
        ui.columns(2, |cols| {
            cols[0].group(|ui| {
                ui.heading("⚡ Power & Diagnostics");
                let has_device = udid.is_some() && self.active_job.is_none();

                if ui.add_enabled(has_device, egui::Button::new("🔄 Restart Device")).clicked() {
                    let u = udid.clone().unwrap();
                    self.start_job(JobKind::RestartDevice, move || match device::restart(&u) {
                        Ok(msg) => JobResult::Completed(msg),
                        Err(err) => JobResult::Failed(err),
                    });
                }
                ui.add_space(4.0);

                if ui.add_enabled(has_device, egui::Button::new("🛑 Shut Down Device")).clicked() {
                    let u = udid.clone().unwrap();
                    self.start_job(JobKind::ShutdownDevice, move || match device::shutdown(&u) {
                        Ok(msg) => JobResult::Completed(msg),
                        Err(err) => JobResult::Failed(err),
                    });
                }
                ui.add_space(4.0);

                if ui.add_enabled(has_device, egui::Button::new("💤 Sleep Device")).clicked() {
                    let u = udid.clone().unwrap();
                    self.start_job(JobKind::RestartDevice, move || match device::sleep(&u) {
                        Ok(msg) => JobResult::Completed(msg),
                        Err(err) => JobResult::Failed(err),
                    });
                }
            });

            cols[1].group(|ui| {
                ui.heading("📸 Screen & Logs");
                ui.label("Screenshot Destination:");
                ui.text_edit_singleline(&mut self.screenshot_path);

                let has_device = udid.is_some() && self.active_job.is_none();
                if ui.add_enabled(has_device, egui::Button::new("Capture Screenshot")).clicked() {
                    let u = udid.clone().unwrap();
                    let dest = PathBuf::from(&self.screenshot_path);
                    self.start_job(JobKind::TakeScreenshot, move || match toolbox::take_screenshot(&u, &dest) {
                        Ok(msg) => JobResult::Completed(msg),
                        Err(err) => JobResult::Failed(err),
                    });
                }

                ui.add_space(8.0);
                ui.label("Crash Reports Folder:");
                ui.text_edit_singleline(&mut self.crash_reports_dir);
                if ui.add_enabled(has_device, egui::Button::new("Export Crash Reports")).clicked() {
                    let u = udid.unwrap();
                    let dest = PathBuf::from(&self.crash_reports_dir);
                    self.start_job(JobKind::CollectCrashReports, move || match toolbox::collect_crash_reports(&u, &dest) {
                        Ok(msg) => JobResult::Completed(msg),
                        Err(err) => JobResult::Failed(err),
                    });
                }
            });
        });
    }

    fn render_activity(&mut self, ui: &mut egui::Ui) {
        ui.heading("Activity Log");
        ui.label("Detailed audit trail of all background operations and commands executed.");
        ui.add_space(10.0);

        ScrollArea::vertical().stick_to_bottom(true).show(ui, |ui| {
            for line in &self.activity {
                ui.monospace(line);
            }
        });
    }

    fn render_about(&mut self, ui: &mut egui::Ui) {
        ui.heading("About iCareFone Clone");
        ui.add_space(10.0);
        ui.label("iCareFone Clone is an independent, clean-room, offline macOS utility for managing iPhones, iPads, and Android devices.");
        ui.label("It is written in 100% safe Rust and released under the MIT License.");
        ui.add_space(10.0);
        ui.label("• Zero proprietary dependencies, zero tracking, zero telemetry, zero analytics.");
        ui.label("• Delegates low-level device protocols to the trusted open-source `libimobiledevice` and `adb` toolchains.");
        ui.label("• Built with egui/eframe 0.33, rusqlite, and aes-gcm.");
    }
}

fn current_time_str() -> String {
    let now = std::time::SystemTime::now();
    let duration = now.duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    let secs = duration.as_secs();
    let hours = (secs / 3600) % 24;
    let mins = (secs / 60) % 60;
    let s = secs % 60;
    format!("{hours:02}:{mins:02}:{s:02}")
}
