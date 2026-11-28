//! Linux `usbdevfs` (`usbfs`) ioctl bindings matching Android NDK `linux/usbdevice_fs.h`.
//!
//! Operates on the raw file descriptor obtained from Android's `UsbDeviceConnection.getFileDescriptor()`.
//! No root privileges or external C libraries are required.

use std::alloc::{Layout, alloc_zeroed, dealloc};
use std::ffi::{c_int, c_uchar, c_uint, c_void};
use std::io;
use std::mem::{align_of, size_of};
use std::os::unix::io::RawFd;
use std::ptr::NonNull;

use crate::timing::UsbSpeed;

// Linux ioctl encoding constants (generic asm-generic/ioctl.h, identical on aarch64, arm, x86_64).
const IOC_NONE: u32 = 0;
const IOC_WRITE: u32 = 1;
const IOC_READ: u32 = 2;
const IOC_NRSHIFT: u32 = 0;
const IOC_TYPESHIFT: u32 = 8;
const IOC_SIZESHIFT: u32 = 16;
const IOC_DIRSHIFT: u32 = 30;

const fn ioc(dir: u32, ty: u8, nr: u8, size: usize) -> u32 {
    (dir << IOC_DIRSHIFT)
        | ((ty as u32) << IOC_TYPESHIFT)
        | ((nr as u32) << IOC_NRSHIFT)
        | ((size as u32) << IOC_SIZESHIFT)
}

const fn io_none(ty: u8, nr: u8) -> u32 {
    ioc(IOC_NONE, ty, nr, 0)
}
const fn ior<T>(ty: u8, nr: u8) -> u32 {
    ioc(IOC_READ, ty, nr, size_of::<T>())
}
const fn iow<T>(ty: u8, nr: u8) -> u32 {
    ioc(IOC_WRITE, ty, nr, size_of::<T>())
}
const fn iowr<T>(ty: u8, nr: u8) -> u32 {
    ioc(IOC_READ | IOC_WRITE, ty, nr, size_of::<T>())
}

#[repr(C)]
pub struct UsbdevfsCtrlTransfer {
    pub b_request_type: u8,
    pub b_request: u8,
    pub w_value: u16,
    pub w_index: u16,
    pub w_length: u16,
    pub timeout: u32,
    pub data: *mut c_void,
}

#[repr(C)]
pub struct UsbdevfsSetInterface {
    pub interface: c_uint,
    pub altsetting: c_uint,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct UsbdevfsIsoPacketDesc {
    pub length: c_uint,
    pub actual_length: c_uint,
    pub status: c_uint,
}

#[repr(C)]
pub struct UsbdevfsUrb {
    pub urb_type: c_uchar,
    pub endpoint: c_uchar,
    pub status: c_int,
    pub flags: c_uint,
    pub buffer: *mut c_void,
    pub buffer_length: c_int,
    pub actual_length: c_int,
    pub start_frame: c_int,
    pub number_of_packets: c_int,
    pub error_count: c_int,
    pub signr: c_uint,
    pub usercontext: *mut c_void,
    // Followed immediately in memory by `number_of_packets` x `UsbdevfsIsoPacketDesc`.
}

#[repr(C)]
pub struct UsbdevfsIoctl {
    pub ifno: c_int,
    pub ioctl_code: c_int,
    pub data: *mut c_void,
}

pub const USBDEVFS_URB_TYPE_ISO: u8 = 0;
pub const USBDEVFS_URB_ISO_ASAP: u32 = 0x02;

// Exact ioctl codes from Android NDK `linux/usbdevice_fs.h`.
// Note: usbdevfs historically encodes several ioctls with inverted _IOR/_IOW directions.
const USBDEVFS_CONTROL: u32 = iowr::<UsbdevfsCtrlTransfer>(b'U', 0);
const USBDEVFS_SETINTERFACE: u32 = ior::<UsbdevfsSetInterface>(b'U', 4);
const USBDEVFS_SUBMITURB: u32 = ior::<UsbdevfsUrb>(b'U', 10);
const USBDEVFS_DISCARDURB: u32 = io_none(b'U', 11);
const USBDEVFS_REAPURBNDELAY: u32 = iow::<*mut c_void>(b'U', 13);
const USBDEVFS_IOCTL: u32 = iowr::<UsbdevfsIoctl>(b'U', 18);
const USBDEVFS_CONNECT: u32 = io_none(b'U', 23);
const USBDEVFS_GET_SPEED: u32 = io_none(b'U', 31);
const USBDEVFS_FORBID_SUSPEND: u32 = io_none(b'U', 33);
const USBDEVFS_ALLOW_SUSPEND: u32 = io_none(b'U', 34);

// Linux `enum usb_device_speed` values returned by USBDEVFS_GET_SPEED.
const LINUX_USB_SPEED_FULL: c_int = 2;
const LINUX_USB_SPEED_HIGH: c_int = 3;
const LINUX_USB_SPEED_SUPER: c_int = 5;
const LINUX_USB_SPEED_SUPER_PLUS: c_int = 6;

fn ioctl_err(rc: c_int) -> io::Result<c_int> {
    if rc < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(rc)
    }
}

/// Sends a synchronous USB control transfer (`EP0`) over `usbfs`.
pub fn control_transfer(
    fd: RawFd,
    request_type: u8,
    request: u8,
    value: u16,
    index: u16,
    data: &mut [u8],
    timeout_ms: u32,
) -> io::Result<usize> {
    let mut ctrl = UsbdevfsCtrlTransfer {
        b_request_type: request_type,
        b_request: request,
        w_value: value,
        w_index: index,
        w_length: data.len().min(u16::MAX as usize) as u16,
        timeout: timeout_ms,
        data: if data.is_empty() {
            std::ptr::null_mut()
        } else {
            data.as_mut_ptr().cast()
        },
    };
    let rc = unsafe { libc::ioctl(fd, USBDEVFS_CONTROL as libc::Ioctl, &mut ctrl) };
    Ok(ioctl_err(rc)? as usize)
}

/// Switches `interface` to alternate setting `altsetting`.
pub fn set_interface(fd: RawFd, interface: u8, altsetting: u8) -> io::Result<()> {
    let arg = UsbdevfsSetInterface {
        interface: c_uint::from(interface),
        altsetting: c_uint::from(altsetting),
    };
    let rc = unsafe { libc::ioctl(fd, USBDEVFS_SETINTERFACE as libc::Ioctl, &arg) };
    ioctl_err(rc)?;
    Ok(())
}

/// Queries the kernel for the device's negotiated USB bus speed (`USBDEVFS_GET_SPEED`, Linux 5.0+).
pub fn query_speed(fd: RawFd) -> Option<UsbSpeed> {
    let rc = unsafe { libc::ioctl(fd, USBDEVFS_GET_SPEED as libc::Ioctl) };
    match rc {
        LINUX_USB_SPEED_FULL => Some(UsbSpeed::Full),
        LINUX_USB_SPEED_HIGH => Some(UsbSpeed::High),
        LINUX_USB_SPEED_SUPER | LINUX_USB_SPEED_SUPER_PLUS => Some(UsbSpeed::Super),
        _ => None,
    }
}

/// Prevents the USB core from autosuspending the DAC while our driver owns it (best-effort).
pub fn forbid_suspend(fd: RawFd) {
    let _ = unsafe { libc::ioctl(fd, USBDEVFS_FORBID_SUSPEND as libc::Ioctl) };
}

/// Re-enables USB autosuspend when releasing the DAC (best-effort).
pub fn allow_suspend(fd: RawFd) {
    let _ = unsafe { libc::ioctl(fd, USBDEVFS_ALLOW_SUSPEND as libc::Ioctl) };
}

/// Asks the kernel to re-attach its driver (`snd-usb-audio`) to `interface` after we release it.
pub fn reconnect_kernel_driver(fd: RawFd, interface: u8) {
    let mut cmd = UsbdevfsIoctl {
        ifno: c_int::from(interface),
        ioctl_code: USBDEVFS_CONNECT as c_int,
        data: std::ptr::null_mut(),
    };
    let _ = unsafe { libc::ioctl(fd, USBDEVFS_IOCTL as libc::Ioctl, &mut cmd) };
}

/// Waits up to `timeout_ms` for any URB completion (`POLLOUT`) or device disconnect (`POLLERR`/`POLLHUP`).
pub fn poll_urb_ready(fd: RawFd, timeout_ms: i32) -> io::Result<i16> {
    let mut pfd = libc::pollfd {
        fd,
        events: libc::POLLOUT | libc::POLLERR | libc::POLLHUP,
        revents: 0,
    };
    let rc = unsafe { libc::poll(&mut pfd, 1, timeout_ms) };
    if rc < 0 {
        let err = io::Error::last_os_error();
        if err.kind() == io::ErrorKind::Interrupted {
            return Ok(0);
        }
        return Err(err);
    }
    Ok(pfd.revents)
}

/// Heap-allocated `usbdevfs_urb` with trailing `iso_frame_desc[num_packets]` and owned DMA data buffer.
pub struct IsoUrb {
    ptr: NonNull<u8>,
    layout: Layout,
    num_packets: usize,
    data: Vec<u8>,
}

// SAFETY: An `IsoUrb` is owned exclusively by the streaming thread once constructed.
unsafe impl Send for IsoUrb {}

impl IsoUrb {
    pub fn new(endpoint: u8, num_packets: usize, buffer_bytes: usize, user_tag: usize) -> Self {
        assert!(num_packets > 0);
        let total_size =
            size_of::<UsbdevfsUrb>() + num_packets * size_of::<UsbdevfsIsoPacketDesc>();
        let align = align_of::<UsbdevfsUrb>().max(align_of::<UsbdevfsIsoPacketDesc>());
        let layout = Layout::from_size_align(total_size, align).expect("valid URB layout");
        let raw = unsafe { alloc_zeroed(layout) };
        let ptr = NonNull::new(raw).expect("URB allocation failed");
        let mut data = vec![0u8; buffer_bytes.max(1)];

        unsafe {
            let urb = ptr.as_ptr().cast::<UsbdevfsUrb>();
            (*urb).urb_type = USBDEVFS_URB_TYPE_ISO;
            (*urb).endpoint = endpoint;
            (*urb).flags = USBDEVFS_URB_ISO_ASAP;
            (*urb).buffer = data.as_mut_ptr().cast();
            (*urb).buffer_length = data.len() as c_int;
            (*urb).number_of_packets = num_packets as c_int;
            (*urb).usercontext = user_tag as *mut c_void;
        }

        Self {
            ptr,
            layout,
            num_packets,
            data,
        }
    }

    pub fn raw_ptr(&self) -> *mut UsbdevfsUrb {
        self.ptr.as_ptr().cast()
    }

    pub fn data_mut(&mut self) -> &mut [u8] {
        &mut self.data
    }

    pub fn data(&self) -> &[u8] {
        &self.data
    }

    pub fn packets_mut(&mut self) -> &mut [UsbdevfsIsoPacketDesc] {
        unsafe {
            let base = self.ptr.as_ptr().add(size_of::<UsbdevfsUrb>()).cast::<UsbdevfsIsoPacketDesc>();
            std::slice::from_raw_parts_mut(base, self.num_packets)
        }
    }

    pub fn packets(&self) -> &[UsbdevfsIsoPacketDesc] {
        unsafe {
            let base = self.ptr.as_ptr().add(size_of::<UsbdevfsUrb>()).cast::<UsbdevfsIsoPacketDesc>();
            std::slice::from_raw_parts(base, self.num_packets)
        }
    }

    pub fn set_buffer_length(&mut self, len: usize) {
        debug_assert!(len <= self.data.len());
        unsafe {
            let urb = self.raw_ptr();
            (*urb).buffer = self.data.as_mut_ptr().cast();
            (*urb).buffer_length = len as c_int;
            (*urb).status = 0;
            (*urb).actual_length = 0;
            (*urb).error_count = 0;
            (*urb).start_frame = 0;
        }
    }

    pub fn status(&self) -> c_int {
        unsafe { (*self.raw_ptr()).status }
    }
}

impl Drop for IsoUrb {
    fn drop(&mut self) {
        unsafe { dealloc(self.ptr.as_ptr(), self.layout) };
    }
}

/// Submits an iso URB to the kernel (`USBDEVFS_SUBMITURB`).
pub fn submit_urb(fd: RawFd, urb: &mut IsoUrb) -> io::Result<()> {
    let rc = unsafe { libc::ioctl(fd, USBDEVFS_SUBMITURB as libc::Ioctl, urb.raw_ptr()) };
    ioctl_err(rc)?;
    Ok(())
}

/// Cancels an in-flight URB (`USBDEVFS_DISCARDURB`). Must still be reaped before freeing!
pub fn discard_urb(fd: RawFd, urb: &IsoUrb) -> io::Result<()> {
    let rc = unsafe { libc::ioctl(fd, USBDEVFS_DISCARDURB as libc::Ioctl, urb.raw_ptr()) };
    ioctl_err(rc)?;
    Ok(())
}

/// Non-blocking reap of one completed URB (`USBDEVFS_REAPURBNDELAY`).
/// Returns `Ok(None)` on `EAGAIN`, `Ok(Some(user_tag))` when an URB completed.
pub fn reap_urb_ndelay(fd: RawFd) -> io::Result<Option<usize>> {
    let mut urb_ptr: *mut UsbdevfsUrb = std::ptr::null_mut();
    let rc = unsafe { libc::ioctl(fd, USBDEVFS_REAPURBNDELAY as libc::Ioctl, &mut urb_ptr) };
    if rc < 0 {
        let err = io::Error::last_os_error();
        if err.raw_os_error() == Some(libc::EAGAIN) {
            return Ok(None);
        }
        return Err(err);
    }
    if urb_ptr.is_null() {
        return Ok(None);
    }
    let tag = unsafe { (*urb_ptr).usercontext as usize };
    Ok(Some(tag))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usbdevfs_struct_sizes_and_ioctl_codes_match_kernel_uapi() {
        // On 64-bit Linux (x86_64 and aarch64), verify exact ABI sizes against NDK usbdevice_fs.h.
        if size_of::<*mut c_void>() == 8 {
            assert_eq!(size_of::<UsbdevfsCtrlTransfer>(), 24);
            assert_eq!(size_of::<UsbdevfsSetInterface>(), 8);
            assert_eq!(size_of::<UsbdevfsIsoPacketDesc>(), 12);
            assert_eq!(size_of::<UsbdevfsUrb>(), 56);
            assert_eq!(size_of::<UsbdevfsIoctl>(), 16);
        }
        assert_eq!(USBDEVFS_GET_SPEED, 0x0000_551F);
        assert_eq!(USBDEVFS_DISCARDURB, 0x0000_550B);
        assert_eq!(USBDEVFS_SETINTERFACE, 0x8008_5504);
    }
}
