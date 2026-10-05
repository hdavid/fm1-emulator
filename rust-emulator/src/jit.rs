// SPDX-License-Identifier: GPL-3.0-only
// Small native backend for prepared register operations. Every entry returns
// after one guest instruction, retaining device/IRQ and dual-core boundaries.
use crate::blocks::Op;
mod aarch64;
mod memory;
mod x86_64;

pub(crate) struct Code {
    _memory: memory::Executable,
    entries: Vec<Option<unsafe extern "C" fn(*mut u32, *mut u32)>>,
}
impl Code {
    pub(crate) fn compile(ops: impl Iterator<Item = Op>) -> Option<Self> {
        let mut bytes = Vec::new();
        let entries = ops
            .map(|op| {
                let previous = bytes.len();
                let start = (previous + 15) & !15;
                // Align indirect targets for Windows control-flow protection.
                // Padding is never executed: each entry ends with a return.
                bytes.resize(start, 0);
                #[cfg(target_arch = "aarch64")]
                let supported = aarch64::emit(op, &mut bytes);
                #[cfg(target_arch = "x86_64")]
                let supported = x86_64::emit(op, &mut bytes, cfg!(windows));
                #[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
                let supported = false;
                if supported {
                    Some(start)
                } else {
                    bytes.truncate(previous);
                    None
                }
            })
            .collect::<Vec<_>>();
        if bytes.is_empty() {
            return None;
        }
        let memory = memory::Executable::new(&bytes)?;
        // Resolve entry addresses once. The allocation is owned alongside this
        // table and every function is exposed only with a borrow of its owner.
        let entries = entries
            .into_iter()
            .map(|offset| offset.map(|offset| unsafe { memory.function(offset) }))
            .collect();
        Some(Self {
            _memory: memory,
            entries,
        })
    }
    pub(crate) fn entry(&self, index: usize) -> Option<Entry<'_>> {
        let function = self.entries.get(index).copied().flatten()?;
        Some(Entry {
            function,
            _owner: std::marker::PhantomData,
        })
    }
}
pub(crate) struct Entry<'a> {
    function: unsafe extern "C" fn(*mut u32, *mut u32),
    _owner: std::marker::PhantomData<&'a Code>,
}
impl Entry<'_> {
    pub(crate) fn run(self, r: &mut [u32; 16], sr: &mut [u32; 16]) {
        // SAFETY: the owner keeps this emitter-produced RX entry alive. The
        // host C ABI and bounded 32-bit accesses only use these exclusive arrays.
        unsafe {
            (self.function)(r.as_mut_ptr(), sr.as_mut_ptr());
        }
    }
}
