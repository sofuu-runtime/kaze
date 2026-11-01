//! Linux GPIO, zero dependencies — Kaze v0.2.
//!
//! sysfs (`/sys/class/gpio`) is the whole implementation: export the line, set
//! its direction, write the value. No ioctl, no `unsafe`, and a script cannot
//! touch a pin the operator did not name (`--pin fan=17`).

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use serde_json::{json, Value};

/// The real sysfs root. Tests point `KAZE_GPIO_ROOT` at a directory instead.
pub const SYSFS_ROOT: &str = "/sys/class/gpio";

/// A named set of GPIO lines.
pub struct GpioBus {
    root: PathBuf,
    pins: HashMap<String, u16>,
    exported: Vec<u16>,
}

impl GpioBus {
    /// `root` of `None` means `$KAZE_GPIO_ROOT`, else `/sys/class/gpio`.
    pub fn new(pins: &[(String, u16)], root: Option<&Path>) -> GpioBus {
        let root = root
            .map(|p| p.to_path_buf())
            .or_else(|| std::env::var_os("KAZE_GPIO_ROOT").map(PathBuf::from))
            .unwrap_or_else(|| PathBuf::from(SYSFS_ROOT));
        GpioBus { root, pins: pins.iter().cloned().collect(), exported: Vec::new() }
    }

    pub fn is_empty(&self) -> bool {
        self.pins.is_empty()
    }

    pub fn pin_of(&self, name: &str) -> Option<u16> {
        self.pins.get(name).copied()
    }

    /// Drive a named line high or low.
    pub fn set(&mut self, name: &str, on: bool) -> Result<(), String> {
        let pin = self.pin_of(name).ok_or_else(|| {
            format!("E_KAZE_LINK: no GPIO pin is configured for '{}' (--pin {}=<number>)", name, name)
        })?;
        let path = self.exported_path(pin)?.join("value");
        write_value(&path, if on { "1" } else { "0" })
            .map_err(|e| format!("E_KAZE_LINK: cannot drive GPIO {} ({}): {}", pin, path.display(), e))
    }

    /// Read a named line.
    pub fn read(&mut self, name: &str) -> Result<bool, String> {
        let pin = self.pin_of(name)
            .ok_or_else(|| format!("E_KAZE_LINK: no GPIO pin is configured for '{}'", name))?;
        let path = self.exported_path(pin)?.join("value");
        let raw = std::fs::read_to_string(&path)
            .map_err(|e| format!("E_KAZE_LINK: cannot read {}: {}", path.display(), e))?;
        Ok(raw.trim() != "0")
    }

    /// Raise the line, then lower it after `seconds` on a background thread, so
    /// neither the script nor the flight loop waits on the timer.
    pub fn pulse(&mut self, name: &str, seconds: f64) -> Result<(), String> {
        let pin = self.pin_of(name)
            .ok_or_else(|| format!("E_KAZE_LINK: no GPIO pin is configured for '{}'", name))?;
        self.set(name, true)?;
        let root = self.root.clone();
        let ms = (seconds.max(0.0) * 1000.0).round() as u64;
        thread::Builder::new()
            .name(format!("kaze-gpio-pulse-{}", pin))
            .spawn(move || {
                thread::sleep(Duration::from_millis(ms));
                let _ = write_value(&root.join(format!("gpio{}", pin)).join("value"), "0");
            })
            .map_err(|e| format!("E_KAZE_LINK: cannot start the pulse timer: {}", e))?;
        Ok(())
    }

    /// The wiring, for a `--json` report.
    pub fn describe(&self) -> Value {
        let pins: Vec<Value> = self
            .pins
            .iter()
            .map(|(name, pin)| json!({ "name": name, "line": pin }))
            .collect();
        json!({ "pins": pins })
    }

    fn exported_path(&mut self, pin: u16) -> Result<PathBuf, String> {
        let dir = self.root.join(format!("gpio{}", pin));
        if dir.join("value").exists() {
            return Ok(dir);
        }
        let export = self.root.join("export");
        write_value(&export, &pin.to_string()).map_err(|e| {
            format!("E_KAZE_LINK: cannot export GPIO {} via {}: {}", pin, export.display(), e)
        })?;
        if !self.exported.contains(&pin) {
            self.exported.push(pin);
        }
        for _ in 0..20 {
            if dir.join("value").exists() {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        let _ = std::fs::write(dir.join("direction"), "out");
        Ok(dir)
    }
}

impl Drop for GpioBus {
    fn drop(&mut self) {
        for pin in self.exported.drain(..) {
            let _ = write_value(&self.root.join("unexport"), &pin.to_string());
        }
    }
}

fn write_value(path: &Path, value: &str) -> std::io::Result<()> {
    let mut file = std::fs::OpenOptions::new().write(true).truncate(false).open(path)?;
    file.write_all(value.as_bytes())?;
    file.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn fake_root(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("kaze-gpio-{}-{}", tag, std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("gpio17")).expect("fake sysfs");
        fs::write(root.join("export"), "").expect("export");
        fs::write(root.join("gpio17").join("value"), "0").expect("value");
        root
    }

    #[test]
    fn a_named_pin_is_driven_through_sysfs() {
        let root = fake_root("drive");
        let mut bus = GpioBus::new(&[("fan".to_string(), 17u16)], Some(&root));
        bus.set("fan", true).expect("pin is configured");
        assert_eq!(fs::read_to_string(root.join("gpio17").join("value")).unwrap(), "1");
        bus.set("fan", false).expect("pin is configured");
        assert_eq!(fs::read_to_string(root.join("gpio17").join("value")).unwrap(), "0");
        assert!(!bus.read("fan").expect("readable"));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn an_unnamed_device_is_refused_not_guessed() {
        let root = fake_root("refuse");
        let mut bus = GpioBus::new(&[("fan".to_string(), 17u16)], Some(&root));
        let err = bus.set("mister", true).expect_err("no pin for mister");
        assert!(err.contains("no GPIO pin is configured for 'mister'"), "{err}");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_pulse_lowers_the_line_by_itself() {
        let root = fake_root("pulse");
        let mut bus = GpioBus::new(&[("fan".to_string(), 17u16)], Some(&root));
        bus.pulse("fan", 0.05).expect("pulse arms");
        assert_eq!(fs::read_to_string(root.join("gpio17").join("value")).unwrap(), "1");
        thread::sleep(Duration::from_millis(200));
        assert_eq!(fs::read_to_string(root.join("gpio17").join("value")).unwrap(), "0");
        let _ = fs::remove_dir_all(&root);
    }
}
