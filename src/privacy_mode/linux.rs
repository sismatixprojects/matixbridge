//! Privacy mode on Linux (X11): the customer's monitors go black while the agent keeps seeing
//! the desktop.
//!
//! It zeroes each active output's brightness with `xrandr --brightness 0`. That scales the CRTC
//! gamma ramp, which the display hardware applies after the framebuffer, so the screen capture
//! (which reads the framebuffer) is unchanged while the panel shows black. Measured on a Linux
//! customer (XFCE, X11) on 2026-11-22: panel black, remote picture normal, restored with
//! `--brightness 1`.
//!
//! Wayland is not supported: capture goes through the portal and xrandr does not drive the
//! outputs there.
//!
//! A screen left black is the failure that matters, so the original brightness of every output
//! is written to a marker file BEFORE anything is blacked out, and restored:
//! on turn-off, on connection close (the connection turns privacy mode off), in `clear`/`Drop`,
//! and when the `--server` process starts again after a crash (`restore_left_black`).
//!
//! Only blacks out the screen. Blocking the customer's mouse and keyboard is not part of it.

use super::{PrivacyMode, PrivacyModeState};
use hbb_common::{anyhow::anyhow, bail, config::Config, lazy_static, log, ResultType};
use std::{path::PathBuf, process::Command};

pub const PRIVACY_MODE_IMPL: &str = "privacy_mode_impl_linux";

/// A saved brightness below this is treated as 1.0, so a restore can never leave a screen black
/// (e.g. a marker written while an output was already blacked out).
const MIN_RESTORE_BRIGHTNESS: f32 = 0.1;

lazy_static::lazy_static! {
    static ref HAS_XRANDR: bool = Command::new("xrandr")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
}

/// X11 session with xrandr available. Checked at call time: `is_x11()` can be wrong early in a
/// boot, and the answer must follow the session the customer is actually in.
pub fn is_supported() -> bool {
    crate::platform::is_x11() && !crate::platform::current_is_wayland() && *HAS_XRANDR
}

fn marker_path() -> PathBuf {
    Config::path("privacy_mode_linux_brightness")
}

fn valid_output_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
}

/// (output, brightness) for every output that has an active CRTC, from `xrandr --verbose`.
/// An output without a CRTC (connected but off) prints no Brightness line and is skipped.
fn parse_outputs(verbose: &str) -> Vec<(String, f32)> {
    let mut out = Vec::new();
    let mut current: Option<String> = None;
    for line in verbose.lines() {
        if !line.starts_with(char::is_whitespace) {
            current = None;
            let mut parts = line.split_whitespace();
            if let (Some(name), Some(state)) = (parts.next(), parts.next()) {
                if state == "connected" && valid_output_name(name) {
                    current = Some(name.to_owned());
                }
            }
            continue;
        }
        if let Some(rest) = line.trim().strip_prefix("Brightness:") {
            if let (Some(name), Ok(b)) = (current.take(), rest.trim().parse::<f32>()) {
                out.push((name, b));
            }
        }
    }
    out
}

fn active_outputs() -> ResultType<Vec<(String, f32)>> {
    let o = Command::new("xrandr").arg("--verbose").output()?;
    if !o.status.success() {
        bail!(
            "xrandr --verbose failed: {}",
            String::from_utf8_lossy(&o.stderr).trim()
        );
    }
    Ok(parse_outputs(&String::from_utf8_lossy(&o.stdout)))
}

fn set_brightness(output: &str, brightness: f32) -> ResultType<()> {
    if !valid_output_name(output) {
        bail!("Invalid output name: {}", output);
    }
    let o = Command::new("xrandr")
        .args(["--output", output, "--brightness", &format!("{:.2}", brightness)])
        .output()?;
    if !o.status.success() {
        bail!(
            "xrandr --output {} --brightness {:.2} failed: {}",
            output,
            brightness,
            String::from_utf8_lossy(&o.stderr).trim()
        );
    }
    Ok(())
}

fn restore_brightness(b: f32) -> f32 {
    if b.is_finite() && b >= MIN_RESTORE_BRIGHTNESS {
        b.min(1.0)
    } else {
        1.0
    }
}

fn write_marker(saved: &[(String, f32)]) -> ResultType<()> {
    let path = marker_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let body: String = saved
        .iter()
        .map(|(n, b)| format!("{} {:.2}\n", n, restore_brightness(*b)))
        .collect();
    std::fs::write(&path, body)?;
    Ok(())
}

fn read_marker() -> Option<Vec<(String, f32)>> {
    let body = std::fs::read_to_string(marker_path()).ok()?;
    Some(
        body.lines()
            .filter_map(|l| {
                let mut p = l.split_whitespace();
                let name = p.next()?;
                let b = p.next()?.parse::<f32>().ok()?;
                valid_output_name(name).then(|| (name.to_owned(), restore_brightness(b)))
            })
            .collect(),
    )
}

/// Restores every output named in the marker, then removes the marker. Returns the first error,
/// after trying every output. The marker is kept if any output could not be restored, so the
/// next start tries again.
fn restore_from_marker() -> ResultType<()> {
    let Some(saved) = read_marker() else {
        return Ok(());
    };
    let mut first_err = None;
    for (name, b) in saved.iter() {
        if let Err(e) = set_brightness(name, *b) {
            log::error!("Privacy mode: failed to restore {}: {}", name, e);
            first_err.get_or_insert(e);
        }
    }
    match first_err {
        Some(e) => Err(e),
        None => {
            let _ = std::fs::remove_file(marker_path());
            Ok(())
        }
    }
}

/// Called when the `--server` process starts: a previous one that died while privacy mode was
/// on left the screen black and its marker behind.
pub fn restore_left_black() {
    if !marker_path().exists() {
        return;
    }
    log::info!("Privacy mode: a previous session left the screen black, restoring");
    if let Err(e) = restore_from_marker() {
        log::error!("Privacy mode: restore at start failed: {}", e);
    }
}

pub struct PrivacyModeImpl {
    impl_key: String,
    conn_id: i32,
}

impl PrivacyModeImpl {
    pub fn new(impl_key: &str) -> Self {
        Self {
            impl_key: impl_key.to_owned(),
            conn_id: 0,
        }
    }
}

impl PrivacyMode for PrivacyModeImpl {
    fn is_async_privacy_mode(&self) -> bool {
        false
    }

    fn init(&self) -> ResultType<()> {
        Ok(())
    }

    fn clear(&mut self) {
        if self.conn_id != 0 || marker_path().exists() {
            if let Err(e) = restore_from_marker() {
                log::error!("Privacy mode: restore on clear failed: {}", e);
            }
        }
        self.conn_id = 0;
    }

    fn turn_on_privacy(&mut self, conn_id: i32) -> ResultType<bool> {
        if self.check_on_conn_id(conn_id)? {
            return Ok(true);
        }
        if !is_supported() {
            bail!("Privacy mode needs an X11 session (Wayland is not supported)");
        }
        // A marker left by a crash belongs to the outputs as they were before it: restore them
        // first, so what is saved now is the real brightness and not 0.
        restore_from_marker()?;
        let outputs = active_outputs()?;
        if outputs.is_empty() {
            bail!(super::NO_PHYSICAL_DISPLAYS);
        }
        write_marker(&outputs)?;
        for (name, _) in outputs.iter() {
            if let Err(e) = set_brightness(name, 0.0) {
                if let Err(e2) = restore_from_marker() {
                    log::error!("Privacy mode: restore after failed turn-on failed: {}", e2);
                }
                return Err(anyhow!("Failed to turn on privacy mode: {}", e));
            }
        }
        log::info!(
            "Privacy mode on for conn {}: {} output(s) blacked out",
            conn_id,
            outputs.len()
        );
        self.conn_id = conn_id;
        Ok(true)
    }

    fn turn_off_privacy(
        &mut self,
        conn_id: i32,
        _state: Option<PrivacyModeState>,
    ) -> ResultType<()> {
        self.check_off_conn_id(conn_id)?;
        restore_from_marker()?;
        if self.conn_id != 0 {
            log::info!("Privacy mode off for conn {}", self.conn_id);
        }
        self.conn_id = 0;
        Ok(())
    }

    fn pre_conn_id(&self) -> i32 {
        self.conn_id
    }

    fn get_impl_key(&self) -> &str {
        &self.impl_key
    }
}

impl Drop for PrivacyModeImpl {
    fn drop(&mut self) {
        self.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const VERBOSE: &str = "Screen 0: minimum 320 x 200, current 3840 x 1080, maximum 16384 x 16384
eDP-1 connected primary 1920x1080+0+0 (0x46) normal (normal left inverted right x axis y axis) 344mm x 194mm
\tIdentifier: 0x42
\tGamma:      1.0:1.0:1.0
\tBrightness: 1.00
  1920x1080 (0x46) 138.700MHz +HSync -VSync *current +preferred
HDMI-1 connected 1920x1080+1920+0 (0x4a) normal (normal left inverted right x axis y axis) 527mm x 296mm
\tBrightness: 0.80
DP-1 disconnected (normal left inverted right x axis y axis)
\tBrightness: 1.00
DP-2 connected (normal left inverted right x axis y axis)
";

    #[test]
    fn parses_only_connected_outputs_with_a_crtc() {
        assert_eq!(
            parse_outputs(VERBOSE),
            vec![("eDP-1".to_owned(), 1.0), ("HDMI-1".to_owned(), 0.8)]
        );
    }

    #[test]
    fn a_restore_never_leaves_a_screen_black() {
        assert_eq!(restore_brightness(0.0), 1.0);
        assert_eq!(restore_brightness(0.05), 1.0);
        assert_eq!(restore_brightness(f32::NAN), 1.0);
        assert_eq!(restore_brightness(0.8), 0.8);
        assert_eq!(restore_brightness(3.0), 1.0);
    }

    #[test]
    fn output_names_are_restricted() {
        assert!(valid_output_name("eDP-1"));
        assert!(valid_output_name("DisplayPort-0"));
        assert!(!valid_output_name("eDP-1; rm -rf /"));
        assert!(!valid_output_name(""));
    }
}
