use std::alloc::{GlobalAlloc, Layout, System};
use std::hint::black_box;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use slime_converter::Dictionary;

struct CountingAllocator;

static COUNTING: AtomicBool = AtomicBool::new(false);
static ALLOCATIONS: AtomicU64 = AtomicU64::new(0);
static ALLOCATED_BYTES: AtomicU64 = AtomicU64::new(0);

// SAFETY: Every operation delegates to the system allocator with the exact
// layout/pointer it received. The atomics only observe successful allocations.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: Delegating the caller-provided layout to System is valid.
        let pointer = unsafe { System.alloc(layout) };
        record_allocation(pointer, layout.size());
        pointer
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // SAFETY: Delegating the caller-provided layout to System is valid.
        let pointer = unsafe { System.alloc_zeroed(layout) };
        record_allocation(pointer, layout.size());
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        // SAFETY: The pointer and layout came from the corresponding System
        // allocation through this allocator.
        unsafe { System.dealloc(pointer, layout) };
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: The pointer/layout pair came from System and new_size is the
        // caller-requested replacement size.
        let replacement = unsafe { System.realloc(pointer, layout, new_size) };
        record_allocation(replacement, new_size);
        replacement
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

fn record_allocation(pointer: *mut u8, bytes: usize) {
    if !pointer.is_null() && COUNTING.load(Ordering::Relaxed) {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        ALLOCATED_BYTES.fetch_add(bytes as u64, Ordering::Relaxed);
    }
}

fn main() {
    let dictionary = Dictionary::bundled();
    let iterations = iterations(1_000);

    run("converter/candidate_window_single_word", iterations, || {
        black_box(dictionary.candidates(black_box("にほん")));
    });
    run("converter/segmented_phrase", iterations, || {
        black_box(dictionary.convert_best(black_box("わたしはにほん")));
    });
    run("converter/n_best_search", iterations, || {
        black_box(dictionary.convert_n_best(black_box("わたしはにほん"), black_box(10)));
    });
    run("converter/n_best_phrase", iterations, || {
        black_box(dictionary.candidates(black_box("わたしはにほん")));
    });
}

fn iterations(default: u64) -> u64 {
    std::env::var("SLIME_BENCH_ITERATIONS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

fn run(name: &str, iterations: u64, mut operation: impl FnMut()) {
    for _ in 0..100 {
        operation();
    }

    ALLOCATIONS.store(0, Ordering::Relaxed);
    ALLOCATED_BYTES.store(0, Ordering::Relaxed);
    COUNTING.store(true, Ordering::Relaxed);
    for _ in 0..iterations {
        operation();
    }
    COUNTING.store(false, Ordering::Relaxed);

    let allocations = ALLOCATIONS.load(Ordering::Relaxed) / iterations;
    let bytes = ALLOCATED_BYTES.load(Ordering::Relaxed) / iterations;
    println!("{name}\t{allocations}\tallocations/op\t{bytes}\tbytes/op");
}
