//! Runtime core: values, GC heap, strings, tables, function objects.

pub mod coroutine;
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
    }
    pub(crate) fn is_freed(p: usize) -> bool {
        FREED.with(|f| f.borrow().contains(&p))
    }
}
pub mod function;
pub mod heap;
pub mod string;
pub mod table;
pub mod userdata;
pub mod value;

pub use coroutine::{Coro, CoroStatus};
pub use function::{
    AfterClose, CallFrame, CloseCont, ContKind, Frame, LocVar, LuaClosure, MetaAction, MetaCont,
    NativeClosure, NativeCont, Proto, UpvalDesc, UpvalState, Upvalue,
};
pub use heap::ObjTag;
pub use heap::{Gc, Heap};
pub use string::LuaStr;
pub use table::{Table, TableError};
pub use userdata::{FileHandle, Userdata, UserdataPayload};
pub use value::Value;
