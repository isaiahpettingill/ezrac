use core::{
    alloc::{GlobalAlloc, Layout},
    arch::asm,
    cell::UnsafeCell,
    ptr,
};

pub struct DpmiAllocator {
    heap: UnsafeCell<Heap>,
}

struct Heap {
    current: usize,
    end: usize,
}

const ARENA_SIZE: usize = 4 * 1024 * 1024;

impl DpmiAllocator {
    pub const fn new() -> Self {
        Self {
            heap: UnsafeCell::new(Heap { current: 0, end: 0 }),
        }
    }
}

// DOS/32A runs this executable on one thread. Interrupt handlers do not call
// into Rust or allocate, so access to the heap is serialized by construction.
unsafe impl Sync for DpmiAllocator {}

unsafe impl GlobalAlloc for DpmiAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let heap = unsafe { &mut *self.heap.get() };
        let align = layout.align();
        let size = layout.size().max(1);

        let Some(end) = align_up(heap.current, align).checked_add(size) else {
            return ptr::null_mut();
        };
        if end > heap.end {
            let arena_size = ARENA_SIZE.max(size.saturating_add(align));
            let Some(base) = allocate_block(arena_size) else {
                return ptr::null_mut();
            };
            let Some(arena_end) = base.checked_add(arena_size) else {
                return ptr::null_mut();
            };
            heap.current = base;
            heap.end = arena_end;
        }

        let pointer = align_up(heap.current, align);
        let Some(next) = pointer.checked_add(size) else {
            return ptr::null_mut();
        };
        if next > heap.end {
            return ptr::null_mut();
        }
        heap.current = next;
        pointer as *mut u8
    }

    unsafe fn dealloc(&self, _pointer: *mut u8, _layout: Layout) {}
}

const fn align_up(value: usize, align: usize) -> usize {
    value.saturating_add(align - 1) & !(align - 1)
}

fn allocate_block(size: usize) -> Option<usize> {
    let size = u32::try_from(size).ok()?;
    let mut address_low: u16;
    let mut address_high: u16;
    let mut failed: u8;
    unsafe {
        asm!(
            "push esi",
            "int 0x31",
            "pop esi",
            "setc {failed}",
            inlateout("ax") 0x0501u16 => _,
            inlateout("bx") (size >> 16) as u16 => address_high,
            inlateout("cx") size as u16 => address_low,
            lateout("di") _,
            failed = lateout(reg_byte) failed,
        );
    }
    if failed != 0 {
        None
    } else {
        Some(((address_high as usize) << 16) | address_low as usize)
    }
}
