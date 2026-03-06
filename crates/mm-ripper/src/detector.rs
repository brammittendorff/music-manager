//! Cross-platform optical drive detection.
//!
//! Linux: uses udev to receive kernel events when a disc is inserted/ejected.
//! Windows: polls drive letters for media presence every N seconds.
//! Both platforms also support a manual override (mm rip manual --drive X).

use anyhow::Result;
use mm_config::AppConfig;
use std::time::Duration;
use tracing::info;

#[derive(Debug, Clone)]
pub enum DriveEvent {
    Inserted(String), // drive path e.g. "/dev/cdrom" or "D:\\"
    Ejected(String),
}

pub struct DriveWatcher {
    #[allow(dead_code)]
    poll_interval: Duration,
    #[cfg(target_os = "linux")]
    inner: LinuxWatcher,
    #[cfg(target_os = "windows")]
    inner: WindowsWatcher,
    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    inner: PollingWatcher,
}

impl DriveWatcher {
    pub fn new(cfg: &AppConfig) -> Result<Self> {
        let poll_interval = Duration::from_secs(cfg.ripper.poll_interval_secs);

        #[cfg(target_os = "linux")]
        let inner = LinuxWatcher::new()?;

        #[cfg(target_os = "windows")]
        let inner = WindowsWatcher::new(poll_interval)?;

        #[cfg(not(any(target_os = "linux", target_os = "windows")))]
        let inner = PollingWatcher::new(poll_interval)?;

        Ok(Self { poll_interval, inner })
    }

    pub async fn next_event(&mut self) -> Result<DriveEvent> {
        self.inner.next_event().await
    }
}

// ─── Linux: udev ─────────────────────────────────────────────────────────────

#[cfg(target_os = "linux")]
struct LinuxWatcher {
    socket: udev::MonitorSocket,
}

#[cfg(target_os = "linux")]
impl LinuxWatcher {
    fn new() -> Result<Self> {
        let socket = udev::MonitorBuilder::new()?
            .match_subsystem("block")?
            .listen()?;
        info!("Linux: udev monitor listening on 'block' subsystem");
        Ok(Self { socket })
    }

    async fn next_event(&mut self) -> Result<DriveEvent> {
        loop {
            tokio::time::sleep(Duration::from_millis(500)).await;

            // MonitorSocket::iter() returns MonitorSocketIter<Item=Event>
            for event in self.socket.iter() {
                let event: udev::Event = event;
                let devtype = event
                    .property_value("DEVTYPE")
                    .and_then(|v: &std::ffi::OsStr| v.to_str())
                    .unwrap_or("");

                if devtype != "disk" {
                    continue;
                }

                let id_type = event
                    .property_value("ID_TYPE")
                    .and_then(|v: &std::ffi::OsStr| v.to_str())
                    .unwrap_or("");

                if id_type != "cd" {
                    continue;
                }

                let path = event
                    .devnode()
                    .and_then(|p: &std::path::Path| p.to_str())
                    .unwrap_or("/dev/cdrom")
                    .to_owned();

                let media_present = event
                    .property_value("ID_CDROM_MEDIA")
                    .and_then(|v: &std::ffi::OsStr| v.to_str())
                    .unwrap_or("0");

                return match event.event_type() {
                    udev::EventType::Add | udev::EventType::Change => {
                        if media_present == "1" {
                            Ok(DriveEvent::Inserted(path))
                        } else {
                            Ok(DriveEvent::Ejected(path))
                        }
                    }
                    udev::EventType::Remove => Ok(DriveEvent::Ejected(path)),
                    _ => continue,
                };
            }
        }
    }
}

// ─── Windows: polling drive letters ──────────────────────────────────────────

#[cfg(target_os = "windows")]
struct WindowsWatcher {
    poll_interval: Duration,
    /// Drive letters with media present in last poll (e.g. "D:\\")
    last_state: std::collections::HashSet<String>,
}

#[cfg(target_os = "windows")]
impl WindowsWatcher {
    fn new(poll_interval: Duration) -> Result<Self> {
        Ok(Self {
            poll_interval,
            last_state: std::collections::HashSet::new(),
        })
    }

    async fn next_event(&mut self) -> Result<DriveEvent> {
        loop {
            tokio::time::sleep(self.poll_interval).await;
            let current = self.optical_drives_with_media()?;

            // Detect insertions
            for drive in &current {
                if !self.last_state.contains(drive) {
                    let event = DriveEvent::Inserted(drive.clone());
                    self.last_state = current;
                    return Ok(event);
                }
            }

            // Detect ejections
            let prev = self.last_state.clone();
            for drive in &prev {
                if !current.contains(drive) {
                    self.last_state = current;
                    return Ok(DriveEvent::Ejected(drive.clone()));
                }
            }

            self.last_state = current;
        }
    }

    fn optical_drives_with_media(&self) -> Result<std::collections::HashSet<String>> {
        use std::collections::HashSet;

        let mut result = HashSet::new();

        // GetLogicalDriveStringsW returns all drives e.g. "C:\\\0D:\\\0\0"
        let mut buf = [0u16; 256];
        let len = unsafe {
            windows::Win32::Storage::FileSystem::GetLogicalDriveStringsW(Some(&mut buf))
        };

        let drives_str = String::from_utf16_lossy(&buf[..len as usize]);

        for drive in drives_str.split('\0').filter(|s| !s.is_empty()) {
            let drive_type = unsafe {
                windows::Win32::Storage::FileSystem::GetDriveTypeW(
                    &windows::core::HSTRING::from(drive),
                )
            };

            // DRIVE_CDROM = 5
            if drive_type == windows::Win32::Storage::FileSystem::DRIVE_CDROM {
                // Check if media is present by trying to open the root
                let path = format!("{drive}.");
                if std::path::Path::new(&path).exists() {
                    result.insert(drive.to_owned());
                }
            }
        }

        Ok(result)
    }
}

// ─── Generic polling fallback ────────────────────────────────────────────────

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
struct PollingWatcher {
    poll_interval: Duration,
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
impl PollingWatcher {
    fn new(poll_interval: Duration) -> Result<Self> {
        Ok(Self { poll_interval })
    }

    async fn next_event(&mut self) -> Result<DriveEvent> {
        loop {
            tokio::time::sleep(self.poll_interval).await;
            // Check common paths
            for path in &["/dev/cdrom", "/dev/sr0", "/dev/dvd"] {
                if std::path::Path::new(path).exists() {
                    return Ok(DriveEvent::Inserted(path.to_string()));
                }
            }
        }
    }
}
