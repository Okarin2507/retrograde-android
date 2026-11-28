//! Dedicated real-time USB isochronous streaming thread.
//!
//! Keeps a pool of `OUT` isochronous URBs (plus optional asynchronous `IN` feedback URBs) in
//! flight over Linux `usbdevfs`, pulling bit-exact wire-formatted PCM from [`PcmRingBuffer`] and
//! advancing `played_frames` as each completed URB returns from the USB host controller.

use std::os::unix::io::RawFd;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread::{self, JoinHandle};

use crate::descriptors::{FeedbackEndpoint, IsoEndpoint};
use crate::ring::PcmRingBuffer;
use crate::timing::{IsoPacer, UsbSpeed, packets_per_sec};
use crate::usbfs::{self, IsoUrb};

/// Configuration for one active isochronous streaming thread.
#[derive(Debug, Clone)]
pub struct IsoStreamParams {
    pub fd: RawFd,
    pub sample_rate: u32,
    pub frame_bytes: usize,
    pub speed: UsbSpeed,
    pub out_ep: IsoEndpoint,
    pub fb_ep: Option<FeedbackEndpoint>,
}

/// Shared lock-free telemetry & control flags between the ExoPlayer thread and USB thread.
pub struct StreamShared {
    pub ring: PcmRingBuffer,
    pub stop: AtomicBool,
    pub eos: AtomicBool,
    pub drained: AtomicBool,
    pub disconnected: AtomicBool,
    pub played_frames: AtomicU64,
    pub submitted_real_frames: AtomicU64,
}

impl StreamShared {
    pub fn new(ring_bytes: usize) -> Self {
        Self {
            ring: PcmRingBuffer::with_capacity(ring_bytes),
            stop: AtomicBool::new(false),
            eos: AtomicBool::new(false),
            drained: AtomicBool::new(false),
            disconnected: AtomicBool::new(false),
            played_frames: AtomicU64::new(0),
            submitted_real_frames: AtomicU64::new(0),
        }
    }
}

/// Handle to a running isochronous USB streaming thread.
pub struct IsoStreamThread {
    shared: Arc<StreamShared>,
    handle: Option<JoinHandle<()>>,
}

const OUT_URB_COUNT: usize = 8;
const FB_URB_COUNT: usize = 2;
const FB_TAG_OFFSET: usize = 1000;

impl IsoStreamThread {
    pub fn start(params: IsoStreamParams, shared: Arc<StreamShared>) -> Self {
        shared.stop.store(false, Ordering::Release);
        shared.drained.store(false, Ordering::Release);
        let thread_shared = Arc::clone(&shared);
        let handle = thread::Builder::new()
            .name("rustygrade-usb-iso".into())
            .spawn(move || run_iso_loop(params, thread_shared))
            .expect("spawn USB iso thread");
        Self {
            shared,
            handle: Some(handle),
        }
    }

    pub fn stop(mut self) {
        self.shared.stop.store(true, Ordering::Release);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

impl Drop for IsoStreamThread {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Release);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

fn elevate_audio_thread_priority() {
    // Request Android's `THREAD_PRIORITY_AUDIO` nice value (-16) on a best-effort basis.
    // Unprivileged processes can always lower priority back or match allowed RLIMIT_NICE.
    unsafe {
        let _ = libc::setpriority(libc::PRIO_PROCESS, 0, -16);
    }
}

fn run_iso_loop(params: IsoStreamParams, shared: Arc<StreamShared>) {
    elevate_audio_thread_priority();

    let pps = packets_per_sec(params.speed, params.out_ep.interval);
    // ~4 ms per URB: 32 packets on HS (8000/s) or 4 packets on FS (1000/s).
    let packets_per_urb = ((pps / 250).clamp(2, 32)) as usize;
    let max_pkt = usize::from(params.out_ep.max_packet_size).max(params.frame_bytes);
    let max_urb_bytes = packets_per_urb * max_pkt;

    let mut pacer = IsoPacer::new(
        params.sample_rate,
        params.speed,
        params.out_ep.interval,
        params.out_ep.max_packet_size,
        params.frame_bytes,
    );

    // URBs live in `Option`s so teardown can deliberately leak (`mem::forget`) any URB the kernel
    // may still reference instead of freeing DMA memory out from under `usbdevfs`.
    let mut out_urbs: Vec<Option<IsoUrb>> = (0..OUT_URB_COUNT)
        .map(|i| {
            Some(IsoUrb::new(
                params.out_ep.address,
                packets_per_urb,
                max_urb_bytes,
                i,
            ))
        })
        .collect();
    let mut out_in_flight = [false; OUT_URB_COUNT];
    let mut out_real_frames = [0u64; OUT_URB_COUNT];

    let mut fb_urbs: Vec<Option<IsoUrb>> = if let Some(fb) = params.fb_ep {
        let fb_pkt_len = usize::from(fb.max_packet_size).clamp(3, 8);
        (0..FB_URB_COUNT)
            .map(|i| Some(IsoUrb::new(fb.address, 1, fb_pkt_len, FB_TAG_OFFSET + i)))
            .collect()
    } else {
        Vec::new()
    };
    let mut fb_in_flight = [false; FB_URB_COUNT];

    // Prime all OUT URBs (and FB URBs if async feedback endpoint exists).
    let mut fatal_disconnect = false;
    for i in 0..OUT_URB_COUNT {
        let Some(urb) = out_urbs[i].as_mut() else {
            continue;
        };
        let real = fill_out_urb(urb, &mut pacer, &shared, params.frame_bytes, false);
        match usbfs::submit_urb(params.fd, urb) {
            Ok(()) => {
                out_in_flight[i] = true;
                out_real_frames[i] = real;
            }
            Err(e) => {
                if is_disconnect_error(&e) {
                    fatal_disconnect = true;
                    break;
                }
            }
        }
    }
    if !fatal_disconnect {
        for (i, slot) in fb_urbs.iter_mut().enumerate() {
            let Some(urb) = slot.as_mut() else { continue };
            prepare_fb_urb(urb);
            match usbfs::submit_urb(params.fd, urb) {
                Ok(()) => fb_in_flight[i] = true,
                Err(e) if is_disconnect_error(&e) => {
                    fatal_disconnect = true;
                    break;
                }
                Err(_) => {}
            }
        }
    }

    let mut trailing_silent_urbs_after_eos = 0usize;

    // Main real-time reap + refill + resubmit loop.
    while !fatal_disconnect && !shared.stop.load(Ordering::Acquire) {
        let revents = match usbfs::poll_urb_ready(params.fd, 15) {
            Ok(ev) => ev,
            Err(_) => {
                // EINTR is retried inside `poll_urb_ready`. Any other poll failure ends this thread,
                // so report the stream as dead: otherwise `start()` sees a live `thread` and
                // `write_pcm` keeps returning 0 forever.
                fatal_disconnect = true;
                break;
            }
        };
        if (revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL)) != 0 {
            fatal_disconnect = true;
            break;
        }

        loop {
            let tag = match usbfs::reap_urb_ndelay(params.fd) {
                Ok(Some(t)) => t,
                Ok(None) => break,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => {
                    if is_disconnect_error(&e) {
                        fatal_disconnect = true;
                    }
                    break;
                }
            };

            if tag >= FB_TAG_OFFSET {
                let idx = tag - FB_TAG_OFFSET;
                let Some(urb) = fb_urbs.get_mut(idx).and_then(Option::as_mut) else {
                    continue;
                };
                fb_in_flight[idx] = false;
                let st = urb.status();
                if st == -libc::ENODEV || st == -libc::ESHUTDOWN {
                    fatal_disconnect = true;
                    break;
                }
                if st == 0 {
                    let pkt = urb.packets()[0];
                    if pkt.status == 0 && pkt.actual_length >= 3 {
                        let len = (pkt.actual_length as usize).min(urb.data().len());
                        pacer.apply_feedback(&urb.data()[..len]);
                    }
                }
                if !shared.stop.load(Ordering::Acquire) {
                    prepare_fb_urb(urb);
                    match usbfs::submit_urb(params.fd, urb) {
                        Ok(()) => fb_in_flight[idx] = true,
                        Err(e) if is_disconnect_error(&e) => {
                            fatal_disconnect = true;
                            break;
                        }
                        Err(_) => {}
                    }
                }
            } else if tag < OUT_URB_COUNT {
                let Some(urb) = out_urbs[tag].as_mut() else {
                    continue;
                };
                // URBs that may still carry real audio: this one plus every other one in flight.
                let active_out = out_in_flight.iter().filter(|&&f| f).count().max(1);
                out_in_flight[tag] = false;
                let completed_real = out_real_frames[tag];
                out_real_frames[tag] = 0;
                if completed_real > 0 {
                    shared
                        .played_frames
                        .fetch_add(completed_real, Ordering::Release);
                }

                // Check for device disconnect signaled via URB status (-ENODEV / -ESHUTDOWN).
                let st = urb.status();
                if st == -libc::ENODEV || st == -libc::ESHUTDOWN {
                    fatal_disconnect = true;
                    break;
                }

                if shared.stop.load(Ordering::Acquire) {
                    continue;
                }

                let eos = shared.eos.load(Ordering::Acquire);
                let count_underrun =
                    !eos && shared.submitted_real_frames.load(Ordering::Relaxed) > 0;
                let real =
                    fill_out_urb(urb, &mut pacer, &shared, params.frame_bytes, count_underrun);
                if eos && real == 0 && shared.ring.available_read() == 0 {
                    trailing_silent_urbs_after_eos += 1;
                    // Once every URB that could still hold real audio has been reaped, mark drained.
                    if trailing_silent_urbs_after_eos >= active_out {
                        shared.drained.store(true, Ordering::Release);
                    }
                } else {
                    trailing_silent_urbs_after_eos = 0;
                }

                match usbfs::submit_urb(params.fd, urb) {
                    Ok(()) => {
                        out_in_flight[tag] = true;
                        out_real_frames[tag] = real;
                    }
                    Err(e) => {
                        if is_disconnect_error(&e) {
                            fatal_disconnect = true;
                            break;
                        }
                    }
                }
            }
        }

        if fatal_disconnect || shared.stop.load(Ordering::Acquire) {
            break;
        }

        // Recover URBs whose last submission failed with a transient (non-disconnect) error, so the
        // pool never silently shrinks to zero and leaves this thread polling forever.
        for i in 0..OUT_URB_COUNT {
            if out_in_flight[i] {
                continue;
            }
            let Some(urb) = out_urbs[i].as_mut() else {
                continue;
            };
            let eos = shared.eos.load(Ordering::Acquire);
            let count_underrun = !eos && shared.submitted_real_frames.load(Ordering::Relaxed) > 0;
            let real = fill_out_urb(urb, &mut pacer, &shared, params.frame_bytes, count_underrun);
            match usbfs::submit_urb(params.fd, urb) {
                Ok(()) => {
                    out_in_flight[i] = true;
                    out_real_frames[i] = real;
                }
                Err(e) => {
                    // The frames were consumed from the ring but never reached the device.
                    if is_disconnect_error(&e) {
                        fatal_disconnect = true;
                        break;
                    }
                }
            }
        }
        if !fatal_disconnect {
            for (i, slot) in fb_urbs.iter_mut().enumerate() {
                if fb_in_flight[i] {
                    continue;
                }
                let Some(urb) = slot.as_mut() else { continue };
                prepare_fb_urb(urb);
                match usbfs::submit_urb(params.fd, urb) {
                    Ok(()) => fb_in_flight[i] = true,
                    Err(e) if is_disconnect_error(&e) => {
                        fatal_disconnect = true;
                        break;
                    }
                    Err(_) => {}
                }
            }
        }
        if !fatal_disconnect && out_in_flight.iter().all(|&f| !f) {
            // Nothing can complete any more: treat as a dead device rather than hanging forever.
            fatal_disconnect = true;
            break;
        }
    }

    if fatal_disconnect {
        shared.disconnected.store(true, Ordering::Release);
    }

    // Teardown: discard every still-in-flight URB and reap it before dropping the URB memory!
    for (i, slot) in out_urbs.iter().enumerate() {
        if let (true, Some(urb)) = (out_in_flight[i], slot.as_ref()) {
            let _ = usbfs::discard_urb(params.fd, urb);
        }
    }
    for (i, slot) in fb_urbs.iter().enumerate() {
        if let (true, Some(urb)) = (fb_in_flight[i], slot.as_ref()) {
            let _ = usbfs::discard_urb(params.fd, urb);
        }
    }

    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(250);
    while (out_in_flight.iter().any(|&f| f) || fb_in_flight.iter().any(|&f| f))
        && std::time::Instant::now() < deadline
    {
        match usbfs::reap_urb_ndelay(params.fd) {
            Ok(Some(tag)) => {
                if tag >= FB_TAG_OFFSET {
                    let idx = tag - FB_TAG_OFFSET;
                    if idx < fb_in_flight.len() {
                        fb_in_flight[idx] = false;
                    }
                } else if tag < OUT_URB_COUNT {
                    out_in_flight[tag] = false;
                }
            }
            Ok(None) => {
                let _ = usbfs::poll_urb_ready(params.fd, 5);
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        }
    }

    // Any URB still in flight may be referenced by usbdevfs: leak it rather than freeing memory
    // the kernel could still DMA into.
    for (i, slot) in out_urbs.iter_mut().enumerate() {
        if out_in_flight[i]
            && let Some(urb) = slot.take()
        {
            std::mem::forget(urb);
        }
    }
    for (i, slot) in fb_urbs.iter_mut().enumerate() {
        if fb_in_flight[i]
            && let Some(urb) = slot.take()
        {
            std::mem::forget(urb);
        }
    }
}

fn fill_out_urb(
    urb: &mut IsoUrb,
    pacer: &mut IsoPacer,
    shared: &StreamShared,
    frame_bytes: usize,
    count_underrun: bool,
) -> u64 {
    let mut total_bytes = 0usize;
    let max_len = urb.data().len();
    for pkt in urb.packets_mut() {
        let mut bytes = (pacer.next_packet_frames() as usize) * frame_bytes;
        if total_bytes + bytes > max_len {
            bytes = ((max_len - total_bytes) / frame_bytes) * frame_bytes;
        }
        pkt.length = bytes as u32;
        pkt.actual_length = 0;
        pkt.status = 0;
        total_bytes += bytes;
    }

    let real_bytes = shared
        .ring
        .read_or_silence(&mut urb.data_mut()[..total_bytes], count_underrun);
    urb.set_buffer_length(total_bytes);
    let real_frames = (real_bytes / frame_bytes) as u64;
    if real_frames > 0 {
        shared.submitted_real_frames.fetch_add(real_frames, Ordering::Relaxed);
    }
    real_frames
}

fn prepare_fb_urb(urb: &mut IsoUrb) {
    let len = urb.data().len();
    for pkt in urb.packets_mut() {
        pkt.length = len as u32;
        pkt.actual_length = 0;
        pkt.status = 0;
    }
    urb.set_buffer_length(len);
}

fn is_disconnect_error(err: &std::io::Error) -> bool {
    matches!(
        err.raw_os_error(),
        Some(libc::ENODEV | libc::ESHUTDOWN | libc::ENOENT | libc::EBADF)
    )
}
