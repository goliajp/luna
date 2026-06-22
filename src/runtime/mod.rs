//! Runtime core: values, GC heap, strings, tables, function objects.

pub mod coroutine;
pub mod frame_marker;
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
