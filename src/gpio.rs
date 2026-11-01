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
    /// Lines already requested from a character device (feature `chardev`).
    #[cfg(feature = "chardev")]
    lines: HashMap<u16, crate::gpio_chardev::ChipLine>,
    root: PathBuf,
    pins: HashMap<String, u16>,
    exported: Vec<u16>,
    /// Cached [`Self::with_chip_probe`] answer: the character-device path this
    /// board would use, so the refusal can name it.
    chip_probe: Option<PathBuf>,
}

impl GpioBus {
    /// `root` of `None` means `$KAZE_GPIO_ROOT`, else `/sys/class/gpio`.
    pub fn new(pins: &[(String, u16)], root: Option<&Path>) -> GpioBus {
        let root = root
            .map(|p| p.to_path_buf())
            .or_else(|| std::env::var_os("KAZE_GPIO_ROOT").map(PathBuf::from))
            .unwrap_or_else(|| PathBuf::from(SYSFS_ROOT));
        GpioBus {
            root,
            pins: pins.iter().cloned().collect(),
            exported: Vec::new(),
            #[cfg(feature = "chardev")]
            lines: HashMap::new(),
            chip_probe: None,
        }
    }

    /// Name the character device the refusal should mention. Tests point this
    /// at a file, since no GPIO chip exists on a laptop.
    pub fn with_chip_probe(mut self, chip: Option<PathBuf>) -> GpioBus {
        self.chip_probe = chip;
        self
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
        #[cfg(feature = "chardev")]
        if self.sysfs_absent() {
            return self.line(pin)?.set(on);
        }
        let path = self.exported_path(pin)?.join("value");
        write_value(&path, if on { "1" } else { "0" })
            .map_err(|e| format!("E_KAZE_LINK: cannot drive GPIO {} ({}): {}", pin, path.display(), e))
    }

    /// Read a named line.
    pub fn read(&mut self, name: &str) -> Result<bool, String> {
        let pin = self.pin_of(name)
            .ok_or_else(|| format!("E_KAZE_LINK: no GPIO pin is configured for '{}'", name))?;
        #[cfg(feature = "chardev")]
        if self.sysfs_absent() {
            return self.line(pin)?.get();
        }
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
        #[cfg(feature = "chardev")]
        if self.sysfs_absent() {
            // The line handle is shared, so the timer thread can lower it
            // without the script waiting or the handle being dropped.
            let line = self.line(pin)?;
            line.set(true)?;
            let ms = (seconds.max(0.0) * 1000.0).round() as u64;
            thread::Builder::new()
                .name(format!("kaze-gpio-pulse-{}", pin))
                .spawn(move || {
                    thread::sleep(Duration::from_millis(ms));
                    let _ = line.set(false);
                })
                .map_err(|e| format!("E_KAZE_LINK: cannot start the pulse timer: {}", e))?;
            return Ok(());
        }
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

    /// Say exactly why this board cannot be driven, which is usually the
    /// difference between a sysfs GPIO interface and the character device one.
    fn no_interface(&self, export: &Path) -> String {
        let chip = self
            .chip_probe
            .clone()
            .or_else(|| std::env::var_os("KAZE_GPIO_CHIP").map(PathBuf::from))
            .unwrap_or_else(|| PathBuf::from("/dev/gpiochip0"));
        if chip.exists() {
            format!(
                "E_KAZE_LINK: GPIO is not usable here: there is no {}, and this board exposes {} (the GPIO character device, which Kaze v0.2 does not drive yet)",
                export.display(),
                chip.display()
            )
        } else {
            format!("E_KAZE_LINK: GPIO is not usable here: there is no {}", export.display())
        }
    }

    /// Is the sysfs interface missing? Then the character device is the way in.
    #[cfg(feature = "chardev")]
    fn sysfs_absent(&self) -> bool {
        !self.root.join("export").exists()
    }

    /// The requested line for `pin`, opening it on first use.
    #[cfg(feature = "chardev")]
    fn line(&mut self, pin: u16) -> Result<crate::gpio_chardev::ChipLine, String> {
        if let Some(line) = self.lines.get(&pin) {
            return Ok(line.clone());
        }
        let chip = self
            .chip_probe
            .clone()
            .or_else(|| std::env::var_os("KAZE_GPIO_CHIP").map(PathBuf::from))
            .unwrap_or_else(|| PathBuf::from("/dev/gpiochip0"));
        let line = crate::gpio_chardev::request_output(&chip, pin)?;
        self.lines.insert(pin, line.clone());
        Ok(line)
    }

    fn exported_path(&mut self, pin: u16) -> Result<PathBuf, String> {
        let dir = self.root.join(format!("gpio{}", pin));
        if dir.join("value").exists() {
            return Ok(dir);
        }
        let export = self.root.join("export");
        if !export.exists() {
            return Err(self.no_interface(&export));
        }
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

    #[cfg(not(feature = "chardev"))]
    #[test]
    fn a_board_with_only_the_character_device_says_so() {
        let root = std::env::temp_dir().join(format!("kaze-gpio-chardev-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("root without export");
        let chip = root.join("gpiochip0");
        fs::write(&chip, "").expect("fake chip node");
        let mut bus = GpioBus::new(&[("fan".to_string(), 17u16)], Some(&root))
            .with_chip_probe(Some(chip.clone()));
        let err = bus.set("fan", true).expect_err("no sysfs here");
        assert!(err.contains("character device"), "{err}");
        assert!(err.contains("gpiochip0"), "the refusal names the interface: {err}");
        let _ = fs::remove_dir_all(&root);
    }

    #[cfg(not(feature = "chardev"))]
    #[test]
    fn a_board_without_gpio_at_all_says_that_instead() {
        let root = std::env::temp_dir().join(format!("kaze-gpio-none-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("empty root");
        let mut bus = GpioBus::new(&[("fan".to_string(), 17u16)], Some(&root))
            .with_chip_probe(Some(root.join("no-such-chip")));
        let err = bus.set("fan", true).expect_err("no GPIO here");
        assert!(err.contains("there is no"), "{err}");
        assert!(!err.contains("character device"), "{err}");
        let _ = fs::remove_dir_all(&root);
    }

    #[cfg(feature = "chardev")]
    #[test]
    fn without_sysfs_the_bus_asks_the_character_device() {
        let root = std::env::temp_dir().join(format!("kaze-gpio-chardev-path-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("root without export");
        let chip = root.join("gpiochip0");
        fs::write(&chip, "").expect("a regular file stands in for the chip");
        let mut bus = GpioBus::new(&[("fan".to_string(), 17u16)], Some(&root))
            .with_chip_probe(Some(chip.clone()));
        let err = bus.set("fan", true).expect_err("a file is not a GPIO chip");
        assert!(
            err.contains("cannot request GPIO 17"),
            "the bus reaches the character device: {err}"
        );
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
