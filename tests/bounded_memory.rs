use std::alloc::{GlobalAlloc, Layout, System};
use std::convert::Infallible;
use std::io::{self, Read};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use rust_strings::{
    scan, Encoding, HitFinish, HitId, HitStart, ScanOptions, SinkControl, StringSink,
};

struct CountingAllocator;

static TRACKING: AtomicBool = AtomicBool::new(false);
static ALLOCATED: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if TRACKING.load(Ordering::Relaxed) {
            ALLOCATED.fetch_add(layout.size(), Ordering::Relaxed);
        }
        // SAFETY: This allocator delegates the unchanged layout to System.
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        // SAFETY: System allocated this pointer with the same layout.
        unsafe { System.dealloc(pointer, layout) }
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if TRACKING.load(Ordering::Relaxed) {
            ALLOCATED.fetch_add(new_size, Ordering::Relaxed);
        }
        // SAFETY: System allocated this pointer, and the layouts are unchanged.
        unsafe { System.realloc(pointer, layout, new_size) }
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

struct DiscardSink;

impl StringSink for DiscardSink {
    type Error = Infallible;

    fn start(&mut self, _id: HitId, _hit: HitStart) -> Result<SinkControl, Self::Error> {
        Ok(SinkControl::Continue)
    }

    fn chunk(&mut self, _id: HitId, _text: &str) -> Result<SinkControl, Self::Error> {
        Ok(SinkControl::Continue)
    }

    fn finish(&mut self, _id: HitId, _hit: HitFinish) -> Result<(), Self::Error> {
        Ok(())
    }

    fn abort(&mut self, _id: HitId) -> Result<(), Self::Error> {
        Ok(())
    }
}

fn allocated_while_scanning(length: u64) -> usize {
    let options = ScanOptions::new(4, [Encoding::ASCII]).unwrap();
    let mut reader = io::repeat(b'A').take(length);
    let mut sink = DiscardSink;
    ALLOCATED.store(0, Ordering::Relaxed);
    TRACKING.store(true, Ordering::Release);
    scan(&mut reader, &options, &mut sink).unwrap();
    TRACKING.store(false, Ordering::Release);
    ALLOCATED.load(Ordering::Relaxed)
}

#[test]
fn scanner_allocations_do_not_scale_with_candidate_size() {
    allocated_while_scanning(16);
    let small = allocated_while_scanning(1024);
    let large = allocated_while_scanning(32 * 1024 * 1024);

    assert_eq!(large, small);
    assert!(large < 64 * 1024, "scanner allocated {large} bytes");
}
