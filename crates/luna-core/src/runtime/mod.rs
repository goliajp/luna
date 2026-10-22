//! Runtime core: values, GC heap, strings, tables, function objects.

mod builtin;
pub mod coroutine;
pub(crate) mod dialect;
pub mod frame_marker;
/// v2.13 WUC `gc-verify` — thread-local log of every freed GcHeader
/// pointer, written by `Heap::free_obj` and consulted by read-time
/// probes (e.g. `Table::find_node`) to catch a dangling reference at
/// the exact dereference site. Exact under quarantining allocators
/// (ASAN); plain allocators may reuse a pointer and mask a hit.
#[cfg(feature = "gc-verify")]
pub(crate) mod gc_verify_probe {
    use std::cell::RefCell;
    use std::collections::HashSet;
    thread_local! {
        pub(crate) static FREED: RefCell<HashSet<usize>> = RefCell::new(HashSet::new());
        /// the bytes of the strings freed, by address
        pub(crate) static FREED_STR: RefCell<std::collections::HashMap<usize, Vec<u8>>> =
            RefCell::new(std::collections::HashMap::new());
    }
    /// The bytes freed string `p` had.
    pub(crate) fn freed_str(p: usize) -> String {
        FREED_STR
            .with(|f| {
                f.borrow()
                    .get(&p)
                    .map(|b| String::from_utf8_lossy(b).into_owned())
            })
            .unwrap_or_default()
    }
    pub(crate) fn is_freed(p: usize) -> bool {
        FREED.with(|f| f.borrow().contains(&p))
    }
}
mod call_frame;
mod debug_info;
mod fnv;
pub mod function;
pub(crate) mod function_close;
#[cfg(feature = "gc-trace")]
pub(crate) mod gc_trace;
pub mod heap;
pub mod mem;
pub(crate) mod puc;
pub mod string;
pub mod table;
mod upvalue;
pub mod userdata;
pub mod value;

pub use builtin::Builtin;
pub use coroutine::{Coro, CoroStatus, ErrorStatus};
pub use function::{
    AfterClose, CallFrame, CloseCont, ContKind, DebugName, Frame, HostCont, LocVar, LuaClosure,
    MetaAction, MetaCont, NativeClosure, NativeCont, Proto, UpvalDesc, UpvalState, Upvalue,
};
pub use heap::ObjTag;
pub use heap::{Gc, GcObject, Heap};
pub use string::LuaStr;
pub use table::{Table, TableError};
pub use userdata::{FileHandle, Userdata, UserdataPayload};
pub use value::Value;
