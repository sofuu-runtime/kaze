//! Linux PWM over sysfs — Kaze v0.2.
//!
//! A channel is named by the operator (`--pwm fan=0:0` is chip 0, channel 0) and
//! driven by a percentage. The period is fixed at 50 Hz, so `set_duty("fan", 60)`
//! means a 60% duty cycle at 20 ms; 0% switches the channel off rather than
//! leaving a channel enabled at zero.
//!
//! Percent is validated by the R7 gate above this module, but the bus checks it
//! again: a value out of range is refused, never clamped, because a clamped duty
//! cycle is a fan running at a speed nobody asked for.

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use serde_json::{json, Value};

/// The real sysfs root. Tests point `KAZE_PWM_ROOT` at a directory instead.
pub const SYSFS_ROOT: &str = "/sys/class/pwm";

/// 50 Hz, in nanoseconds.
pub const PERIOD_NS: u64 = 20_000_000;

/// Named PWM channels.
pub struct PwmBus {
    root: PathBuf,
    channels: HashMap<String, (u16, u16)>,
    exported: Vec<(u16, u16)>,
    /// The last duty written per channel, for the run report.
    last: HashMap<String, f64>,
}

impl PwmBus {
    /// `channels` is `name -> (chip, channel)`. `root` of `None` means
    /// `$KAZE_PWM_ROOT`, else `/sys/class/pwm`.
    pub fn new(channels: &[(String, u16, u16)], root: Option<&Path>) -> PwmBus {
        let root = root
            .map(|p| p.to_path_buf())
            .or_else(|| std::env::var_os("KAZE_PWM_ROOT").map(PathBuf::from))
            .unwrap_or_else(|| PathBuf::from(SYSFS_ROOT));
        PwmBus {
            root,
            channels: channels
                .iter()
                .map(|(name, chip, channel)| (name.clone(), (*chip, *channel)))
                .collect(),
            exported: Vec::new(),
            last: HashMap::new(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.channels.is_empty()
    }

    pub fn channel_of(&self, name: &str) -> Option<(u16, u16)> {
        self.channels.get(name).copied()
    }

    /// The duty cycle last written for a channel.
    pub fn duty_of(&self, name: &str) -> Option<f64> {
        self.last.get(name).copied()
    }

    /// Set a channel to `percent` (0..=100). 0% disables the channel.
    pub fn set_duty(&mut self, name: &str, percent: f64) -> Result<(), String> {
        let (chip, channel) = self.channel_of(name).ok_or_else(|| {
            format!(
                "E_KAZE_LINK: no PWM channel is configured for '{}' (--pwm {}=<chip>:<channel>)",
                name, name
            )
        })?;
        if !percent.is_finite() || !(0.0..=100.0).contains(&percent) {
            return Err(format!(
                "E_KAZE_LINK: {}% is not a duty cycle for '{}' (0 to 100)",
                percent, name
            ));
        }

        let dir = self.exported_dir(chip, channel)?;
        write_value(&dir.join("period"), &PERIOD_NS.to_string()).map_err(|e| {
            format!("E_KAZE_LINK: cannot set the PWM period for '{}': {}", name, e)
        })?;
        let duty = (PERIOD_NS as f64 * percent / 100.0).round() as u64;
        write_value(&dir.join("duty_cycle"), &duty.to_string()).map_err(|e| {
            format!("E_KAZE_LINK: cannot set the PWM duty for '{}': {}", name, e)
        })?;
        write_value(&dir.join("enable"), if percent > 0.0 { "1" } else { "0" }).map_err(|e| {
            format!("E_KAZE_LINK: cannot enable the PWM channel for '{}': {}", name, e)
        })?;

        self.last.insert(name.to_string(), percent);
        Ok(())
    }

    /// The wiring, for a `--json` report.
    pub fn describe(&self) -> Value {
        let channels: Vec<Value> = self
            .channels
            .iter()
            .map(|(name, (chip, channel))| {
                json!({ "name": name, "chip": chip, "channel": channel })
            })
            .collect();
        json!({ "channels": channels, "period_ns": PERIOD_NS })
    }

    fn exported_dir(&mut self, chip: u16, channel: u16) -> Result<PathBuf, String> {
        let chip_dir = self.root.join(format!("pwmchip{}", chip));
        let dir = chip_dir.join(format!("pwm{}", channel));
        if dir.exists() {
            return Ok(dir);
        }
        let export = chip_dir.join("export");
        if !export.exists() {
            return Err(format!(
                "E_KAZE_LINK: PWM is not usable here: there is no {} (is the pwm overlay loaded?)",
                export.display()
            ));
        }
        write_value(&export, &channel.to_string()).map_err(|e| {
            format!(
                "E_KAZE_LINK: cannot export PWM channel {} of chip {}: {}",
                channel, chip, e
            )
        })?;
        for _ in 0..20 {
            if dir.join("duty_cycle").exists() {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        if self.exported.iter().all(|(c, n)| (*c, *n) != (chip, channel)) {
            self.exported.push((chip, channel));
        }
        Ok(dir)
    }
}

impl Drop for PwmBus {
    fn drop(&mut self) {
        // Leave the hardware as it was found: a channel this process turned on
        // does not keep running after the script ends.
        for (chip, channel) in self.exported.drain(..) {
            let dir = self.root.join(format!("pwmchip{}", chip)).join(format!("pwm{}", channel));
            let _ = write_value(&dir.join("enable"), "0");
            let _ = write_value(
                &self.root.join(format!("pwmchip{}", chip)).join("unexport"),
                &channel.to_string(),
            );
        }
    }
}

fn write_value(path: &Path, value: &str) -> std::io::Result<()> {
    // Truncate so a shorter value leaves no stale digits (a fake sysfs file in a
    // test, or any ordinary file). A real sysfs attribute ignores the flag and
    // parses whatever was written, so the fallback keeps device files working.
    let mut file = match std::fs::OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(path)
    {
        Ok(file) => file,
        Err(_) => std::fs::OpenOptions::new()
            .write(true)
            .truncate(false)
            .open(path)?,
    };
    file.write_all(value.as_bytes())?;
    file.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn fake_root(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("kaze-pwm-{}-{}", tag, std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("pwmchip0").join("pwm0")).expect("fake pwm sysfs");
        fs::write(root.join("pwmchip0").join("export"), "").expect("export");
        for file in ["period", "duty_cycle", "enable"] {
            fs::write(root.join("pwmchip0").join("pwm0").join(file), "0").expect("channel file");
        }
        root
    }

    fn read(root: &Path, file: &str) -> String {
        fs::read_to_string(root.join("pwmchip0").join("pwm0").join(file))
            .expect("channel file is readable")
            .trim()
            .to_string()
    }

    #[test]
    fn a_percentage_becomes_a_duty_cycle_at_fifty_hertz() {
        let root = fake_root("duty");
        let mut bus = PwmBus::new(&[("fan".to_string(), 0u16, 0u16)], Some(&root));
        bus.set_duty("fan", 60.0).expect("the channel is configured");
        assert_eq!(read(&root, "period"), PERIOD_NS.to_string());
        assert_eq!(read(&root, "duty_cycle"), "12000000");
        assert_eq!(read(&root, "enable"), "1");
        assert_eq!(bus.duty_of("fan"), Some(60.0));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn zero_percent_switches_the_channel_off() {
        let root = fake_root("off");
        let mut bus = PwmBus::new(&[("fan".to_string(), 0u16, 0u16)], Some(&root));
        bus.set_duty("fan", 40.0).expect("on");
        bus.set_duty("fan", 0.0).expect("off");
        assert_eq!(read(&root, "enable"), "0");
        assert_eq!(read(&root, "duty_cycle"), "0");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn an_out_of_range_duty_is_refused_not_clamped() {
        let root = fake_root("range");
        let mut bus = PwmBus::new(&[("fan".to_string(), 0u16, 0u16)], Some(&root));
        let err = bus.set_duty("fan", 140.0).expect_err("140% is not a duty cycle");
        assert!(err.contains("is not a duty cycle for 'fan'"), "{err}");
        let err = bus.set_duty("fan", f64::NAN).expect_err("NaN is not a duty cycle");
        assert!(err.contains("is not a duty cycle"), "{err}");
        assert_eq!(read(&root, "enable"), "0", "nothing was switched on");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_channel_the_operator_never_named_is_refused() {
        let root = fake_root("unnamed");
        let mut bus = PwmBus::new(&[("fan".to_string(), 0u16, 0u16)], Some(&root));
        let err = bus.set_duty("pump", 50.0).expect_err("no channel for pump");
        assert!(err.contains("no PWM channel is configured for 'pump'"), "{err}");
        assert!(err.contains("--pwm pump=<chip>:<channel>"), "{err}");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_board_without_pwm_says_so() {
        let root = std::env::temp_dir().join(format!("kaze-pwm-none-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("pwmchip0")).expect("chip dir");
        let mut bus = PwmBus::new(&[("fan".to_string(), 0u16, 0u16)], Some(&root));
        let err = bus.set_duty("fan", 50.0).expect_err("no export file");
        assert!(err.contains("PWM is not usable here"), "{err}");
        assert!(err.contains("overlay"), "the refusal hints at the fix: {err}");
        let _ = fs::remove_dir_all(&root);
    }
}
