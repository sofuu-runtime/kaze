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
    /// Pins this bus drove high, so a normal exit leaves them low.
    raised: Vec<u16>,
    /// Names whose de-assert is registered with the signal cleanup table.
    cleanup_armed: Vec<String>,
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
            raised: Vec::new(),
            cleanup_armed: Vec::new(),
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
    ///
    /// Raising a line first arms its abnormal-termination cleanup: a pin whose
    /// de-assert cannot be guaranteed is not allowed to go high.
    pub fn set(&mut self, name: &str, on: bool) -> Result<(), String> {
        let pin = self.pin_of(name).ok_or_else(|| {
            format!("E_KAZE_LINK: no GPIO pin is configured for '{}' (--pin {}=<number>)", name, name)
        })?;
        #[cfg(feature = "chardev")]
        if self.sysfs_absent() {
            let line = self.line(pin)?;
            if on {
                self.arm_line_cleanup(name, &line)?;
            }
            line.set(on)?;
            self.note_line(pin, on);
            return Ok(());
        }
        let path = self.exported_path(pin)?.join("value");
        if on {
            self.arm_value_cleanup(name, &path)?;
        }
        write_value(&path, if on { "1" } else { "0" })
            .map_err(|e| format!("E_KAZE_LINK: cannot drive GPIO {} ({}): {}", pin, path.display(), e))?;
        self.note_line(pin, on);
        Ok(())
    }

    /// Remember which pins are high, so a normal exit lowers them.
    fn note_line(&mut self, pin: u16, on: bool) {
        if on {
            if !self.raised.contains(&pin) {
                self.raised.push(pin);
            }
        } else {
            self.raised.retain(|p| *p != pin);
        }
    }

    /// Register the sysfs `value` descriptor with the signal cleanup table,
    /// once per pin. Registration happens before the drive, so a pin is never
    /// higher than its cleanup coverage.
    fn arm_value_cleanup(&mut self, name: &str, value_path: &Path) -> Result<(), String> {
        if self.cleanup_armed.iter().any(|armed| armed == name) {
            return Ok(());
        }
        cleanup::install_signal_handlers()?;
        let file = std::fs::OpenOptions::new()
            .write(true)
            .open(value_path)
            .map_err(|e| {
                format!(
                    "E_KAZE_LINK: cannot arm the cleanup for '{}' ({}): {}; the line was not driven",
                    name,
                    value_path.display(),
                    e
                )
            })?;
        cleanup::register_zero_sink(file)?;
        self.cleanup_armed.push(name.to_string());
        Ok(())
    }

    /// Register a requested character-device line with the cleanup table.
    #[cfg(feature = "chardev")]
    fn arm_line_cleanup(
        &mut self,
        name: &str,
        line: &crate::gpio_chardev::ChipLine,
    ) -> Result<(), String> {
        if self.cleanup_armed.iter().any(|armed| armed == name) {
            return Ok(());
        }
        cleanup::install_signal_handlers()?;
        cleanup::register_line(line.raw_fd())?;
        self.cleanup_armed.push(name.to_string());
        Ok(())
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
            self.arm_line_cleanup(name, &line)?;
            line.set(true)?;
            self.note_line(pin, true);
            let ms = (seconds.max(0.0) * 1000.0).round() as u64;
            thread::Builder::new()
                .name(format!("kaze-gpio-pulse-{}", pin))
                .spawn(move || {
                    thread::sleep(Duration::from_millis(ms));
                    if let Err(e) = line.set(false) {
                        eprintln!("[kaze:actuator_safety] CRITICAL: pulse timer failed to de-assert pin {}: {}", pin, e);
                    }
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
                if let Err(e) = write_value(&root.join(format!("gpio{}", pin)).join("value"), "0") {
                    eprintln!("[kaze:actuator_safety] CRITICAL: pulse timer failed to de-assert sysfs gpio{}: {}", pin, e);
                }
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
        // Normal exit: lower every line this bus raised before releasing it, so
        // a relay is not left energised just because the script ended politely.
        for pin in &self.raised {
            #[cfg(feature = "chardev")]
            if let Some(line) = self.lines.get(pin) {
                let _ = line.set(false);
            }
            let _ = write_value(&self.root.join(format!("gpio{}", pin)).join("value"), "0");
        }
        for pin in self.exported.drain(..) {
            let _ = write_value(&self.root.join("unexport"), &pin.to_string());
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

/// Abnormal-termination cleanup (Phase 5, F-09).
///
/// A process killed by `SIGINT`/`SIGTERM` never runs `Drop`, so a relay held
/// high stays high. This table holds pre-opened actuator descriptors and one
/// signal handler de-asserts each of them with operations that are safe inside
/// a signal handler: a `write(2)` of `"0"` to a sysfs `value` or PWM
/// `enable` descriptor, or the line ioctl for a requested character device.
/// After the cleanup the handler restores the default action and re-raises the
/// signal, so the process still ends from the signal the operator sent.
///
/// This mirrors the C substrate in `kaze/dist/kaze_rt.{h,c}`
/// (`kaze_install_cleanup_signal_handlers`, `kaze_register_deassert`,
/// `kaze_run_cleanups`); the Rust engine needs the same guarantee for its own
/// process.
#[cfg(unix)]
pub mod cleanup {
    use std::os::fd::{IntoRawFd, RawFd};
    use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU8, AtomicUsize, Ordering};

    /// The C runtime's `KAZE_MAX_DEASSERTS`. A pin that cannot be covered by
    /// the handler is not driven at all, so exceeding this refuses actuation
    /// rather than leaving an actuator the process cannot clean up.
    pub const MAX_SLOTS: usize = 16;

    const SLOT_EMPTY: u8 = 0;
    const SLOT_WRITE_ZERO: u8 = 1;
    const SLOT_CHARDEV: u8 = 2;

    static SLOT_KIND: [AtomicU8; MAX_SLOTS] = [const { AtomicU8::new(SLOT_EMPTY) }; MAX_SLOTS];
    static SLOT_FD: [AtomicI32; MAX_SLOTS] = [const { AtomicI32::new(-1) }; MAX_SLOTS];
    static SLOTS_USED: AtomicUsize = AtomicUsize::new(0);
    static INSTALLED: AtomicBool = AtomicBool::new(false);

    const SIGINT: i32 = 2;
    const SIGTERM: i32 = 15;

    extern "C" {
        fn signal(signum: i32, handler: Option<extern "C" fn(i32)>) -> usize;
        fn raise(sig: i32) -> i32;
        fn write(fd: i32, buf: *const u8, count: usize) -> isize;
        fn dup(fd: i32) -> i32;
        fn _exit(status: i32) -> !;
    }

    extern "C" fn on_cleanup_signal(sig: i32) {
        run_cleanups();
        // Put the default action back and re-raise. The signal is blocked while
        // this handler runs, so the raise stays pending; it is delivered the
        // moment the handler returns, before the interrupted code resumes, and
        // the process dies from the signal exactly as it would have without a
        // handler. `_exit` is the fallback for a platform that refuses the
        // raise, so the process never continues with its actuators lowered.
        unsafe {
            signal(sig, None);
            if raise(sig) != 0 {
                _exit(128 + sig);
            }
        }
    }

    fn store_slot(kind: u8, fd: RawFd) -> Result<usize, String> {
        let index = SLOTS_USED.load(Ordering::SeqCst);
        if index >= MAX_SLOTS {
            return Err(format!(
                "E_KAZE_LINK: all {} actuator-cleanup slots are in use; the line was left de-asserted and not driven",
                MAX_SLOTS
            ));
        }
        SLOT_KIND[index].store(kind, Ordering::SeqCst);
        SLOT_FD[index].store(fd, Ordering::SeqCst);
        SLOTS_USED.store(index + 1, Ordering::SeqCst);
        Ok(index)
    }

    /// Register an already-open descriptor that de-asserts by writing `"0"`
    /// (a sysfs `value` file or a PWM `enable` file). Ownership transfers to
    /// the cleanup table, which holds the descriptor until the process exits.
    pub fn register_zero_sink(file: std::fs::File) -> Result<usize, String> {
        store_slot(SLOT_WRITE_ZERO, file.into_raw_fd())
    }

    /// Register a requested GPIO line. The descriptor is duplicated so the
    /// cleanup table owns its own reference for the life of the process.
    pub fn register_line(fd: RawFd) -> Result<usize, String> {
        if fd < 0 {
            return Err(
                "E_KAZE_LINK: a GPIO line without a descriptor cannot be cleaned up".to_string(),
            );
        }
        let copy = unsafe { dup(fd) };
        if copy < 0 {
            return Err(format!(
                "E_KAZE_LINK: cannot duplicate the GPIO line descriptor for cleanup: {}",
                std::io::Error::last_os_error()
            ));
        }
        store_slot(SLOT_CHARDEV, copy)
    }

    /// De-assert every registered actuator. Public and idempotent so a test can
    /// run exactly the code a signal runs, without sending one.
    pub fn run_cleanups() {
        let used = SLOTS_USED.load(Ordering::SeqCst).min(MAX_SLOTS);
        for slot in 0..used {
            run_cleanup_at(slot);
        }
    }

    /// De-assert one slot. Exposed so a test can exercise the exact signal-time
    /// code without touching the actuators another test registered.
    pub fn run_cleanup_at(slot: usize) {
        let fd = match SLOT_FD.get(slot) {
            Some(fd) => fd.load(Ordering::SeqCst),
            None => return,
        };
        if fd < 0 {
            return;
        }
        match SLOT_KIND[slot].load(Ordering::SeqCst) {
            SLOT_WRITE_ZERO => {
                const ZERO: &[u8] = b"0";
                // write(2) is async-signal-safe.
                unsafe {
                    write(fd, ZERO.as_ptr(), ZERO.len());
                }
            }
            #[cfg(feature = "chardev")]
            SLOT_CHARDEV => crate::gpio_chardev::deassert(fd),
            _ => {}
        }
    }

    /// Install the SIGINT/SIGTERM handlers once. The refusal names the fix so a
    /// caller can fail closed rather than drive a pin it cannot de-assert.
    pub fn install_signal_handlers() -> Result<(), String> {
        if INSTALLED.load(Ordering::SeqCst) {
            return Ok(());
        }
        for sig in [SIGINT, SIGTERM] {
            let previous = unsafe { signal(sig, Some(on_cleanup_signal)) };
            if previous == usize::MAX {
                return Err(format!(
                    "E_KAZE_LINK: cannot install the SIGINT/SIGTERM actuator cleanup handler (signal {} failed); a pin that cannot be de-asserted on abnormal termination is not driven",
                    sig
                ));
            }
        }
        INSTALLED.store(true, Ordering::SeqCst);
        Ok(())
    }

    /// How many actuators the handler would de-assert.
    pub fn registered() -> usize {
        SLOTS_USED.load(Ordering::SeqCst).min(MAX_SLOTS)
    }
}

/// Platforms without POSIX signals cannot install an abnormal-termination
/// handler, so actuation is refused instead of being reported as cleaned up.
#[cfg(not(unix))]
pub mod cleanup {
    pub const MAX_SLOTS: usize = 0;

    pub fn register_zero_sink(_file: std::fs::File) -> Result<usize, String> {
        Err("E_KAZE_LINK: actuator cleanup on abnormal termination needs POSIX signals; this platform has none, so the pin is not driven".to_string())
    }

    pub fn register_line(_fd: i32) -> Result<usize, String> {
        Err("E_KAZE_LINK: actuator cleanup on abnormal termination needs POSIX signals; this platform has none, so the line is not driven".to_string())
    }

    pub fn run_cleanups() {}

    pub fn run_cleanup_at(_slot: usize) {}

    pub fn install_signal_handlers() -> Result<(), String> {
        Err("E_KAZE_LINK: actuator cleanup on abnormal termination needs POSIX signals; this platform has none".to_string())
    }

    pub fn registered() -> usize {
        0
    }
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
    fn the_cleanup_table_de_asserts_a_registered_descriptor() {
        // The exact code a SIGINT/SIGTERM handler runs: write "0" through the
        // descriptor registered when the pin was armed.
        let dir = std::env::temp_dir().join(format!("kaze-gpio-cleanup-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("cleanup dir");
        let path = dir.join("value");
        fs::write(&path, "1").expect("value file");
        let file = std::fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .expect("open the value file");
        let slot = cleanup::register_zero_sink(file).expect("a cleanup slot");
        cleanup::run_cleanup_at(slot);
        assert_eq!(fs::read_to_string(&path).unwrap(), "0");
        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(feature = "chardev")]
    #[test]
    fn a_character_device_descriptor_is_duplicated_into_the_cleanup_table() {
        // No GPIO chip exists here, so this exercises the registration and the
        // de-assert attempt (an ioctl the kernel refuses with ENOTTY, which the
        // handler deliberately ignores) rather than a real line.
        use std::os::fd::AsRawFd;
        let path = std::env::temp_dir().join(format!("kaze-gpio-chardev-cleanup-{}", std::process::id()));
        fs::write(&path, "").expect("stand-in descriptor");
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .expect("open the stand-in");
        let before = cleanup::registered();
        let slot = cleanup::register_line(file.as_raw_fd()).expect("a duplicated descriptor");
        assert!(cleanup::registered() > before);
        cleanup::run_cleanup_at(slot);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn driving_a_pin_registers_its_cleanup_before_it_goes_high() {
        let root = fake_root("armed");
        let before = cleanup::registered();
        let mut bus = GpioBus::new(&[("fan".to_string(), 17u16)], Some(&root));
        bus.set("fan", true).expect("pin is configured");
        assert_eq!(fs::read_to_string(root.join("gpio17").join("value")).unwrap(), "1");
        assert!(
            cleanup::registered() > before,
            "a raised pin must be covered by the signal cleanup table"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn dropping_the_bus_leaves_a_raised_line_low() {
        let root = fake_root("exit");
        {
            let mut bus = GpioBus::new(&[("fan".to_string(), 17u16)], Some(&root));
            bus.set("fan", true).expect("pin is configured");
            assert_eq!(fs::read_to_string(root.join("gpio17").join("value")).unwrap(), "1");
        }
        assert_eq!(
            fs::read_to_string(root.join("gpio17").join("value")).unwrap(),
            "0",
            "a normal exit de-asserts the line"
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
