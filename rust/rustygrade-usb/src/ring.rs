//! Lock-free single-producer single-consumer (SPSC) byte ring buffer for feeding the
//! real-time USB isochronous thread from the ExoPlayer audio thread with zero mutex contention
//! on the hot path.

use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

/// Pads (and aligns) a value to its own 64-byte cache line so the producer-owned `write_pos` and
/// consumer-owned `read_pos` never share a line (no false sharing between the two cores).
#[repr(align(64))]
struct CachePadded<T>(T);

impl<T> std::ops::Deref for CachePadded<T> {
    type Target = T;
    #[inline]
    fn deref(&self) -> &T {
        &self.0
    }
}

/// Fixed-capacity SPSC byte ring buffer (capacity is always rounded up to a power of two).
pub struct PcmRingBuffer {
    buf: Box<[UnsafeCell<u8>]>,
    mask: usize,
    read_pos: CachePadded<AtomicUsize>,
    write_pos: CachePadded<AtomicUsize>,
    underruns: AtomicU64,
}

// SAFETY: Single producer writes only to slots between `write_pos` and `read_pos + capacity`,
// single consumer reads only from slots between `read_pos` and `write_pos`, synchronized via
// Acquire/Release atomics on `read_pos` and `write_pos`.
unsafe impl Sync for PcmRingBuffer {}
unsafe impl Send for PcmRingBuffer {}

impl PcmRingBuffer {
    pub fn with_capacity(min_capacity: usize) -> Self {
        let cap = min_capacity.max(4096).next_power_of_two();
        let mut v = Vec::with_capacity(cap);
        for _ in 0..cap {
            v.push(UnsafeCell::new(0));
        }
        Self {
            buf: v.into_boxed_slice(),
            mask: cap - 1,
            read_pos: CachePadded(AtomicUsize::new(0)),
            write_pos: CachePadded(AtomicUsize::new(0)),
            underruns: AtomicU64::new(0),
        }
    }

    pub fn capacity(&self) -> usize {
        self.mask + 1
    }

    pub fn available_read(&self) -> usize {
        let w = self.write_pos.load(Ordering::Acquire);
        let r = self.read_pos.load(Ordering::Relaxed);
        w.wrapping_sub(r)
    }

    pub fn available_write(&self) -> usize {
        self.capacity() - self.available_read()
    }

    pub fn underruns(&self) -> u64 {
        self.underruns.load(Ordering::Relaxed)
    }

    /// Producer: writes as many bytes from `src` as fit without blocking. Returns bytes written.
    pub fn write(&self, src: &[u8]) -> usize {
        let r = self.read_pos.load(Ordering::Acquire);
        let w = self.write_pos.load(Ordering::Relaxed);
        let free = self.capacity().wrapping_sub(w.wrapping_sub(r));
        let n = src.len().min(free);
        if n == 0 {
            return 0;
        }
        let start = w & self.mask;
        let first = n.min(self.capacity() - start);
        // SAFETY: `[start..start+first]` and `[0..n-first]` are exclusively owned by the producer
        // until `write_pos` is advanced with `Release`.
        unsafe {
            let base = self.buf.as_ptr() as *mut u8;
            std::ptr::copy_nonoverlapping(src.as_ptr(), base.add(start), first);
            if n > first {
                std::ptr::copy_nonoverlapping(src.as_ptr().add(first), base, n - first);
            }
        }
        self.write_pos.store(w.wrapping_add(n), Ordering::Release);
        n
    }

    /// Consumer (real-time audio thread): reads up to `dst.len()` bytes. If fewer bytes are
    /// available, zero-fills the remainder and increments `underruns` (when `count_underrun`).
    /// Returns the number of real PCM bytes read (excluding zero-padding).
    pub fn read_or_silence(&self, dst: &mut [u8], count_underrun: bool) -> usize {
        let w = self.write_pos.load(Ordering::Acquire);
        let r = self.read_pos.load(Ordering::Relaxed);
        let avail = w.wrapping_sub(r);
        let n = dst.len().min(avail);
        if n > 0 {
            let start = r & self.mask;
            let first = n.min(self.capacity() - start);
            // SAFETY: `[start..start+first]` and `[0..n-first]` were published by the producer
            // before `write_pos` was stored with `Release`.
            unsafe {
                let base = self.buf.as_ptr() as *const u8;
                std::ptr::copy_nonoverlapping(base.add(start), dst.as_mut_ptr(), first);
                if n > first {
                    std::ptr::copy_nonoverlapping(base, dst.as_mut_ptr().add(first), n - first);
                }
            }
            self.read_pos.store(r.wrapping_add(n), Ordering::Release);
        }
        if n < dst.len() {
            dst[n..].fill(0);
            if count_underrun {
                self.underruns.fetch_add(1, Ordering::Relaxed);
            }
        }
        n
    }

    /// Clears all buffered audio (called only when the streaming consumer is paused/stopped).
    pub fn clear(&self) {
        let w = self.write_pos.load(Ordering::Acquire);
        self.read_pos.store(w, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wraps_around_cleanly_and_tracks_underruns() {
        let ring = PcmRingBuffer::with_capacity(4096);
        assert_eq!(ring.capacity(), 4096);

        let chunk: Vec<u8> = (0..3000).map(|i| (i % 251) as u8).collect();
        assert_eq!(ring.write(&chunk), 3000);
        assert_eq!(ring.available_read(), 3000);
        assert_eq!(ring.available_write(), 1096);

        let mut out = vec![0u8; 2000];
        assert_eq!(ring.read_or_silence(&mut out, true), 2000);
        assert_eq!(out, chunk[..2000]);
        assert_eq!(ring.underruns(), 0);

        // Write across wrap boundary.
        let chunk2: Vec<u8> = (0..2500).map(|i| ((i * 7) % 253) as u8).collect();
        assert_eq!(ring.write(&chunk2), 2500);
        assert_eq!(ring.available_read(), 3500);

        let mut out2 = vec![0u8; 4000];
        assert_eq!(ring.read_or_silence(&mut out2, true), 3500);
        assert_eq!(&out2[..1000], &chunk[2000..]);
        assert_eq!(&out2[1000..3500], &chunk2[..]);
        assert!(out2[3500..].iter().all(|&b| b == 0));
        assert_eq!(ring.underruns(), 1);
    }
}
