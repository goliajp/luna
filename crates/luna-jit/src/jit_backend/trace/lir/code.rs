//! From a [`Lir`] to executable memory.

use super::*;

/// The baseline tier's code of one `Vm`: chunks of pages, each trace on
/// pages of its own that turn read-execute once written, so no page that
/// may run is ever written again. Freed only by [`CodeArena::free`];
/// dropping the arena alone leaves the code mapped, as with the Cranelift
/// tier's modules, because some `Vm` may still point into it.
#[derive(Default)]
pub(crate) struct CodeArena {
    chunks: std::mem::ManuallyDrop<Vec<region::Allocation>>,
    /// Writable bytes left at the end of the last chunk, from `next`.
    next: usize,
    left: usize,
}

// SAFETY: the arena owns its mappings; the code in them is only run by the
// thread that owns the Vm the arena belongs to
unsafe impl Send for CodeArena {}

const CHUNK: usize = 256 * 1024;

/// Chunks of Vms that released their code, writable again: the next Vm
/// takes one instead of mapping and faulting in fresh pages.
static SPARE_CHUNKS: std::sync::Mutex<Vec<Spare>> = std::sync::Mutex::new(Vec::new());

struct Spare(region::Allocation);

// SAFETY: a spare chunk is plain writable memory nothing points into; the
// pool hands it to one arena at a time
unsafe impl Send for Spare {}
const MAX_SPARE: usize = 8;

impl CodeArena {
    /// Copies `code` to fresh pages and makes them executable.
    fn place(&mut self, code: &[u8]) -> Result<*const u8, &'static str> {
        let page = region::page::size();
        let len = code.len().next_multiple_of(page);
        if len > self.left {
            let spare = if len <= CHUNK {
                SPARE_CHUNKS
                    .lock()
                    .ok()
                    .and_then(|mut v| v.pop())
                    .map(|c| c.0)
            } else {
                None
            };
            let a = match spare {
                Some(a) => a,
                None => region::alloc(len.max(CHUNK), region::Protection::READ_WRITE)
                    .map_err(|_| "allocating code memory")?,
            };
            self.next = a.as_ptr::<u8>() as usize;
            self.left = a.len();
            self.chunks.push(a);
        }
        let p = self.next as *mut u8;
        // SAFETY: `p..p + len` is writable memory of the last chunk that
        // nothing has been placed in yet
        unsafe {
            std::ptr::copy_nonoverlapping(code.as_ptr(), p, code.len());
            region::protect(p, len, region::Protection::READ_EXECUTE)
                .map_err(|_| "protecting code memory")?;
            crate::jit_backend::code_memory::sync_icache(p as usize, code.len());
        }
        self.next += len;
        self.left -= len;
        Ok(p)
    }

    /// # Safety
    ///
    /// None of the code is running or will be entered again.
    pub(crate) unsafe fn free(&mut self) {
        let chunks = std::mem::take(&mut *self.chunks);
        self.left = 0;
        let mut spare = SPARE_CHUNKS.lock().ok();
        for a in chunks {
            // SAFETY: no code in the chunk runs any more (the caller's
            // contract); a chunk that cannot be made writable is unmapped
            let writable = unsafe {
                region::protect(a.as_ptr::<u8>(), a.len(), region::Protection::READ_WRITE).is_ok()
            };
            match spare.as_mut() {
                Some(v) if writable && a.len() == CHUNK && v.len() < MAX_SPARE => v.push(Spare(a)),
                _ => drop(a),
            }
        }
    }
}

#[cfg(all(target_arch = "aarch64", not(windows)))]
type Target = super::a64::A64;
#[cfg(target_arch = "x86_64")]
type Target = super::x64::X64;

/// Generates the code for `lir` into `arena`; `Err` names what the
/// baseline tier does not handle, and the caller compiles with Cranelift
/// instead.
#[cfg(all(
    any(target_arch = "aarch64", target_arch = "x86_64"),
    not(all(windows, target_arch = "aarch64"))
))]
pub(crate) fn assemble(lir: &Lir, arena: &mut CodeArena) -> Result<*const u8, &'static str> {
    use super::cg::Masm;
    if let Some(u) = lir.unsupported {
        return Err(u);
    }
    let mut w = WORK.with(|w| w.borrow_mut().take()).unwrap_or_default();
    let Work { an, al, masm, cg } = &mut *w;
    live::analyze(lir, an);
    alloc::allocate(lir, an, [&Target::INT, &Target::FLT], al);
    let out = cg::generate(lir, an, al, Target::new(std::mem::take(masm)), cg)?;
    dump::write(lir, an, al, &out.bytes);
    let entry = arena.place(&out.bytes);
    *masm = out;
    WORK.with(|x| *x.borrow_mut() = Some(w));
    entry
}

/// The backend's buffers, kept from one trace to the next on a thread.
#[derive(Default)]
struct Work {
    an: live::Analysis,
    al: alloc::Allocation,
    masm: cg::Bufs,
    cg: cg::CgBufs,
}

thread_local! {
    static WORK: std::cell::RefCell<Option<Box<Work>>> = const { std::cell::RefCell::new(None) };
}

#[cfg(not(all(
    any(target_arch = "aarch64", target_arch = "x86_64"),
    not(all(windows, target_arch = "aarch64"))
)))]
pub(crate) fn assemble(_lir: &Lir, _arena: &mut CodeArena) -> Result<*const u8, &'static str> {
    Err("no baseline code generator for this target")
}
