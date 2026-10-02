//! From a [`Lir`] to executable memory.

use super::*;
use crate::jit_backend::code_memory::CodeMemory;
use cranelift_jit::{BranchProtection, JITMemoryKind, JITMemoryProvider};

/// The machine code of one baseline trace. The memory stays mapped until
/// [`BaselineCode::free`]; dropping the value alone leaks it, as with the
/// Cranelift tier's modules, because a trace may still be on the stack.
pub(crate) struct BaselineCode {
    mem: CodeMemory,
    pub(crate) entry: *const u8,
}

impl BaselineCode {
    /// # Safety
    ///
    /// The code is not running and will not be entered again.
    pub(crate) unsafe fn free(mut self) {
        // SAFETY: forwarded from the caller
        unsafe { self.mem.free_memory() }
    }
}

// SAFETY: the memory is owned by this value alone, and the pointer into it
// is only dereferenced by calling the code, which the owning Vm's thread
// does; nothing here is tied to the thread that allocated it.
unsafe impl Send for BaselineCode {}

#[cfg(target_arch = "aarch64")]
type Target = super::a64::A64;
#[cfg(target_arch = "x86_64")]
type Target = super::x64::X64;

/// Generates and maps the code for `lir`; `Err` names what the baseline
/// tier does not handle, and the caller compiles with Cranelift instead.
#[cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]
pub(crate) fn assemble(lir: &Lir) -> Result<BaselineCode, &'static str> {
    use super::cg::Masm;
    if let Some(u) = lir.unsupported {
        return Err(u);
    }
    let an = live::analyze(lir);
    let al = alloc::allocate(lir, &an, [&Target::INT, &Target::FLT]);
    let bytes = cg::generate(lir, &an, &al, Target::new())?;
    dump::write(lir, &an, &al, &bytes);
    let mut mem = CodeMemory::new();
    let p = mem
        .allocate(bytes.len(), 16, JITMemoryKind::Executable)
        .map_err(|_| "allocating code memory")?;
    // SAFETY: `p` is a fresh writable allocation of `bytes.len()` bytes
    unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), p, bytes.len()) };
    mem.finalize(BranchProtection::None)
        .map_err(|_| "protecting code memory")?;
    Ok(BaselineCode { mem, entry: p })
}

#[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
pub(crate) fn assemble(_lir: &Lir) -> Result<BaselineCode, &'static str> {
    Err("no baseline code generator for this target")
}
