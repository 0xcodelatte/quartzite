//! The few places where Linux and the BSDs differ. Everything else goes
//! through libseat (sessions), libinput (input) and DRM/KMS (display),
//! which all three systems provide.

use std::path::PathBuf;

/// Human-readable OS name for logs.
#[cfg(target_os = "linux")]
pub const OS_NAME: &str = "Linux";
#[cfg(target_os = "freebsd")]
pub const OS_NAME: &str = "FreeBSD";
#[cfg(target_os = "openbsd")]
pub const OS_NAME: &str = "OpenBSD";
#[cfg(not(any(target_os = "linux", target_os = "freebsd", target_os = "openbsd")))]
pub const OS_NAME: &str = "unknown Unix";

/// DRM card nodes to try when udev cannot name a primary GPU.
/// drm-kmod on FreeBSD exposes the same `/dev/dri/cardN` layout as Linux.
#[allow(dead_code)]
pub fn fallback_drm_cards() -> Vec<PathBuf> {
    let mut cards: Vec<PathBuf> = std::fs::read_dir("/dev/dri")
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("card"))
        })
        .collect();
    cards.sort();
    cards
}

/// Warn about OS settings libinput needs before we try to start on a TTY.
#[cfg(target_os = "freebsd")]
pub fn check_environment() {
    // libinput on FreeBSD reads evdev nodes, which are off by default for
    // some drivers. `kern.evdev.rcpt_mask=12` routes mice/keyboards there.
    let out = std::process::Command::new("sysctl")
        .args(["-n", "kern.evdev.rcpt_mask"])
        .output();
    if let Ok(out) = out {
        let mask = String::from_utf8_lossy(&out.stdout);
        if mask.trim().parse::<u32>().map(|m| m & 12 != 12).unwrap_or(false) {
            tracing::warn!("kern.evdev.rcpt_mask is {mask}; set it to 12 so libinput sees your devices");
        }
    }
}

#[cfg(not(target_os = "freebsd"))]
pub fn check_environment() {}
