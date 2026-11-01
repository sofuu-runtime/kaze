//! Linux GPIO through the character device (`/dev/gpiochipN`) — Kaze v0.2.
//!
//! Boards that dropped sysfs (Raspberry Pi 5 among them) expose GPIO only this
//! way, so a pin nothing else can drive lives here. The ABI is the documented
//! v2 UAPI: request one line as an output, then read or write its value. There is
//! no event handling, no debounce and no interrupt path — a script can set a
//! line high, low, and read it back.
//!
//! Off by default: build with `--features chardev`. It compiles against the
//! published UAPI and is exercised by the tests below, but has not been run
//! against a real GPIO chip in this project's CI, so treat first use on a board
//! as a check rather than a given.

use std::fs::OpenOptions;
use std::os::fd::{AsRawFd, OwnedFd};
use std::os::raw::{c_int, c_ulong};
use std::os::unix::fs::OpenOptionsExt;
use std::sync::Arc;

/// `_IOC(dir, type, nr, size)` from `asm-generic/ioctl.h`, with the GPIO type.
const fn ioc(dir: u32, ty: u32, nr: u32, size: usize) -> c_ulong {
    ((dir << 30) | ((size as u32) << 16) | (ty << 8) | nr) as c_ulong
}

const IOC_READ_WRITE: u32 = 3;
const GPIO_TYPE: u32 = 0xB4;

/// `GPIO_V2_GET_LINE_IOCTL`.
pub const GPIO_V2_GET_LINE: c_ulong = ioc(IOC_READ_WRITE, GPIO_TYPE, 0x07, std::mem::size_of::<LineRequest>());
/// `GPIO_V2_LINE_GET_VALUES_IOCTL`.
pub const GPIO_V2_LINE_GET_VALUES: c_ulong =
    ioc(IOC_READ_WRITE, GPIO_TYPE, 0x0E, std::mem::size_of::<LineValues>());
/// `GPIO_V2_LINE_SET_VALUES_IOCTL`.
pub const GPIO_V2_LINE_SET_VALUES: c_ulong =
    ioc(IOC_READ_WRITE, GPIO_TYPE, 0x0F, std::mem::size_of::<LineValues>());

const GPIO_V2_LINES_MAX: usize = 64;
const GPIO_MAX_NAME_SIZE: usize = 32;
/// `GPIO_V2_LINE_FLAG_OUTPUT`.
const FLAG_OUTPUT: u64 = 1 << 3;

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct LineValues {
    pub bits: u64,
    pub mask: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct LineAttribute {
    pub id: u32,
    pub padding: u32,
    pub value: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct LineConfigAttribute {
    pub attr: LineAttribute,
    pub mask: u64,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct LineConfig {
    pub flags: u64,
    pub num_attrs: u32,
    pub padding: [u32; 5],
    pub attrs: [LineConfigAttribute; GPIO_V2_LINES_MAX],
}

impl Default for LineConfig {
    fn default() -> Self {
        LineConfig {
            flags: 0,
            num_attrs: 0,
            padding: [0; 5],
            attrs: [LineConfigAttribute::default(); GPIO_V2_LINES_MAX],
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct LineRequest {
    pub offsets: [u32; GPIO_V2_LINES_MAX],
    pub consumer: [u8; GPIO_MAX_NAME_SIZE],
    pub config: LineConfig,
    pub num_lines: u32,
    pub event_buffer_size: u32,
    pub padding: [u32; 5],
    pub fd: i32,
}

impl Default for LineRequest {
    fn default() -> Self {
        LineRequest {
            offsets: [0; GPIO_V2_LINES_MAX],
            consumer: [0; GPIO_MAX_NAME_SIZE],
            config: LineConfig::default(),
            num_lines: 0,
            event_buffer_size: 0,
            padding: [0; 5],
            fd: -1,
        }
    }
}

extern "C" {
    fn ioctl(fd: c_int, request: c_ulong, ...) -> c_int;
}

/// One requested GPIO line, owned by this process.
#[derive(Clone, Debug)]
pub struct ChipLine {
    fd: Arc<OwnedFd>,
    pub pin: u16,
}

impl ChipLine {
    /// Drive the line high or low.
    pub fn set(&self, on: bool) -> Result<(), String> {
        let mut values = LineValues { bits: if on { 1 } else { 0 }, mask: 1 };
        let rc = unsafe { ioctl(self.fd.as_raw_fd(), GPIO_V2_LINE_SET_VALUES, &mut values) };
        if rc < 0 {
            return Err(format!(
                "E_KAZE_LINK: the kernel refused to set GPIO {} ({}): {}",
                self.pin,
                GPIO_V2_LINE_SET_VALUES,
                std::io::Error::last_os_error()
            ));
        }
        Ok(())
    }

    /// Read the line back.
    pub fn get(&self) -> Result<bool, String> {
        let mut values = LineValues { bits: 0, mask: 1 };
        let rc = unsafe { ioctl(self.fd.as_raw_fd(), GPIO_V2_LINE_GET_VALUES, &mut values) };
        if rc < 0 {
            return Err(format!(
                "E_KAZE_LINK: the kernel refused to read GPIO {}: {}",
                self.pin,
                std::io::Error::last_os_error()
            ));
        }
        Ok(values.bits & values.mask != 0)
    }
}

/// Open `chip` and request `offset` as an output, ready to drive.
pub fn request_output(chip: &std::path::Path, offset: u16) -> Result<ChipLine, String> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(0)
        .open(chip)
        .map_err(|e| {
            format!(
                "E_KAZE_LINK: cannot open the GPIO character device {}: {}",
                chip.display(),
                e
            )
        })?;

    let mut request = LineRequest::default();
    request.offsets[0] = offset as u32;
    request.num_lines = 1;
    request.config.flags = FLAG_OUTPUT;
    let name = b"kaze";
    request.consumer[..name.len()].copy_from_slice(name);

    let rc = unsafe { ioctl(file.as_raw_fd(), GPIO_V2_GET_LINE, &mut request) };
    if rc < 0 {
        return Err(format!(
            "E_KAZE_LINK: cannot request GPIO {} from {}: {}",
            offset,
            chip.display(),
            std::io::Error::last_os_error()
        ));
    }
    if request.fd < 0 {
        return Err(format!("E_KAZE_LINK: the kernel returned no handle for GPIO {}", offset));
    }

    // The ioctl hands back a descriptor this process now owns.
    let fd = unsafe { <OwnedFd as std::os::fd::FromRawFd>::from_raw_fd(request.fd) };
    Ok(ChipLine { fd: Arc::new(fd), pin: offset })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_request_matches_the_documented_uapi_size() {
        // sizeof(struct gpio_v2_line_request) on a 64-bit kernel.
        assert_eq!(std::mem::size_of::<LineRequest>(), 1888);
        assert_eq!(std::mem::size_of::<LineConfig>(), 1568);
        assert_eq!(std::mem::size_of::<LineConfigAttribute>(), 24);
        assert_eq!(std::mem::size_of::<LineValues>(), 16);
    }

    #[test]
    fn the_ioctl_numbers_are_the_ones_linux_publishes() {
        assert_eq!(GPIO_V2_GET_LINE, 0xC760B407);
        assert_eq!(GPIO_V2_LINE_GET_VALUES, 0xC010B40E);
        assert_eq!(GPIO_V2_LINE_SET_VALUES, 0xC010B40F);
    }

    #[test]
    fn a_missing_chip_is_refused_by_name() {
        let err = request_output(std::path::Path::new("/dev/kaze-no-such-gpiochip"), 3)
            .expect_err("no chip at that path");
        assert!(err.contains("cannot open the GPIO character device"), "{err}");
    }

    #[test]
    fn a_plain_file_is_not_a_gpio_chip() {
        let path = std::env::temp_dir().join(format!("kaze-fake-chip-{}", std::process::id()));
        std::fs::write(&path, "").expect("write a stand-in");
        let err = request_output(&path, 3).expect_err("a regular file has no lines");
        assert!(err.contains("cannot request GPIO 3"), "{err}");
        let _ = std::fs::remove_file(&path);
    }
}
