// SPDX-License-Identifier: GPL-3.0-only
// Own executable pages separately from the Rust heap. Write once, then RX;
// never retain writable executable memory. Allocation denial uses the interpreter.
use std::ffi::c_void;
const CAPACITY: usize = 16 * 1024;
pub(super) struct Executable {
    ptr: *mut u8,
}
// SAFETY: this allocation has one owner, is immutable after creation, contains
// no pointers into thread-local state, and is released only by its owner.
unsafe impl Send for Executable {}
impl Executable {
    pub(super) fn new(bytes: &[u8]) -> Option<Self> {
        if bytes.is_empty() || bytes.len() > CAPACITY {
            return None;
        }
        #[cfg(any(target_os = "linux", target_os = "macos", windows))]
        unsafe {
            let ptr = os::allocate();
            if ptr.is_null() {
                return None;
            }
            let memory = Self { ptr };
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr, bytes.len());
            if !os::seal(ptr, CAPACITY) {
                return None;
            }
            Some(memory)
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
        None
    }
    pub(super) unsafe fn function(
        &self,
        offset: usize,
    ) -> unsafe extern "C" fn(*mut u32, *mut u32) {
        assert!(offset < CAPACITY);
        let function: unsafe extern "C" fn(*mut u32, *mut u32) =
            std::mem::transmute(self.ptr.add(offset));
        function
    }
}
impl Drop for Executable {
    fn drop(&mut self) {
        #[cfg(any(target_os = "linux", target_os = "macos", windows))]
        unsafe {
            os::release(self.ptr);
        }
    }
}
#[cfg(any(target_os = "linux", target_os = "macos"))]
mod os {
    use super::*;
    extern "C" {
        fn mmap(
            address: *mut c_void,
            length: usize,
            protection: i32,
            flags: i32,
            fd: i32,
            offset: i64,
        ) -> *mut c_void;
        fn mprotect(address: *mut c_void, length: usize, protection: i32) -> i32;
        fn munmap(address: *mut c_void, length: usize) -> i32;
        #[cfg(target_os = "macos")]
        fn sys_icache_invalidate(address: *mut c_void, length: usize);
    }
    pub(super) unsafe fn allocate() -> *mut u8 {
        let flags = if cfg!(target_os = "macos") {
            0x1002
        } else {
            0x22
        }; // PRIVATE | ANONYMOUS
        let ptr = mmap(std::ptr::null_mut(), CAPACITY, 3, flags, -1, 0);
        if ptr as isize == -1 {
            std::ptr::null_mut()
        } else {
            ptr.cast()
        }
    }
    pub(super) unsafe fn seal(ptr: *mut u8, length: usize) -> bool {
        if mprotect(ptr.cast(), length, 5) != 0 {
            return false;
        } // READ | EXECUTE
        #[cfg(target_os = "macos")]
        sys_icache_invalidate(ptr.cast(), length);
        #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
        {
            // Arm's required D-cache clean, barrier, I-cache invalidate sequence.
            // CTR_EL0 reports line sizes in units of four bytes.
            let ctr: u64;
            std::arch::asm!("mrs {ctr}, ctr_el0", ctr = out(reg) ctr, options(nostack, preserves_flags));
            let dline = 4usize << ((ctr >> 16) & 15);
            let iline = 4usize << (ctr & 15);
            let end = ptr as usize + length;
            for address in ((ptr as usize & !(dline - 1))..end).step_by(dline) {
                std::arch::asm!("dc cvau, {address}", address = in(reg) address, options(nostack, preserves_flags));
            }
            std::arch::asm!("dsb ish", options(nostack, preserves_flags));
            for address in ((ptr as usize & !(iline - 1))..end).step_by(iline) {
                std::arch::asm!("ic ivau, {address}", address = in(reg) address, options(nostack, preserves_flags));
            }
            std::arch::asm!("dsb ish", "isb", options(nostack, preserves_flags));
        }
        true
    }
    pub(super) unsafe fn release(ptr: *mut u8) {
        munmap(ptr.cast(), CAPACITY);
    }
}
#[cfg(windows)]
mod os {
    use super::*;
    #[link(name = "kernel32")]
    extern "system" {
        fn VirtualAlloc(
            address: *mut c_void,
            length: usize,
            allocation: u32,
            protection: u32,
        ) -> *mut c_void;
        fn VirtualProtect(
            address: *mut c_void,
            length: usize,
            protection: u32,
            old: *mut u32,
        ) -> i32;
        fn VirtualFree(address: *mut c_void, length: usize, kind: u32) -> i32;
        fn GetCurrentProcess() -> *mut c_void;
        fn FlushInstructionCache(
            process: *mut c_void,
            address: *const c_void,
            length: usize,
        ) -> i32;
    }
    pub(super) unsafe fn allocate() -> *mut u8 {
        VirtualAlloc(std::ptr::null_mut(), CAPACITY, 0x3000, 4).cast()
    }
    pub(super) unsafe fn seal(ptr: *mut u8, length: usize) -> bool {
        let mut old = 0;
        VirtualProtect(ptr.cast(), length, 0x20, &mut old) != 0
            && FlushInstructionCache(GetCurrentProcess(), ptr.cast(), length) != 0
    }
    pub(super) unsafe fn release(ptr: *mut u8) {
        VirtualFree(ptr.cast(), 0, 0x8000);
    }
}
