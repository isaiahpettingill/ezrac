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
    free: *mut FreeBlock,
}

struct FreeBlock {
    size: usize,
    next: *mut FreeBlock,
}

const ARENA_SIZE: usize = 1024 * 1024;
const BLOCK_ALIGN: usize = core::mem::align_of::<FreeBlock>();
const BLOCK_SIZE: usize = core::mem::size_of::<FreeBlock>();

impl DpmiAllocator {
    pub const fn new() -> Self {
        Self {
            heap: UnsafeCell::new(Heap {
                current: 0,
                end: 0,
                free: ptr::null_mut(),
            }),
        }
    }
}

// DOS/32A runs this executable on one thread. Interrupt handlers do not call
// into Rust or allocate, so access to the heap is serialized by construction.
unsafe impl Sync for DpmiAllocator {}

unsafe impl GlobalAlloc for DpmiAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let heap = unsafe { &mut *self.heap.get() };
        let align = layout.align().max(BLOCK_ALIGN);
        let size = align_up(layout.size().max(BLOCK_SIZE), BLOCK_ALIGN);

        let mut previous: *mut FreeBlock = ptr::null_mut();
        let mut block = heap.free;
        while !block.is_null() {
            let fits = unsafe { (*block).size >= size } && (block as usize).is_multiple_of(align);
            if fits {
                let next = unsafe { (*block).next };
                if previous.is_null() {
                    heap.free = next;
                } else {
                    unsafe { (*previous).next = next };
                }
                return block.cast();
            }
            previous = block;
            block = unsafe { (*block).next };
        }

        let Some(start) = align_up(heap.current, align).checked_add(size) else {
            return ptr::null_mut();
        };
        if start > heap.end {
            let arena_size = ARENA_SIZE.max(size.saturating_add(align));
            let Some(base) = allocate_block(arena_size) else {
                return ptr::null_mut();
            };
            heap.current = base;
            heap.end = base + arena_size;
        }

        let pointer = align_up(heap.current, align);
        heap.current = pointer + size;
        pointer as *mut u8
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        if pointer.is_null() {
            return;
        }
        let heap = unsafe { &mut *self.heap.get() };
        let block = pointer.cast::<FreeBlock>();
        unsafe {
            block.write(FreeBlock {
                size: align_up(layout.size().max(BLOCK_SIZE), BLOCK_ALIGN),
                next: heap.free,
            });
        }
        heap.free = block;
    }
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
