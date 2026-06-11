//! Runtime core: values, GC heap, strings, tables, function objects.

pub mod function;
pub mod heap;
pub mod string;
pub mod table;
pub mod value;

pub use function::{LuaClosure, Proto, UpvalDesc, UpvalState, Upvalue};
pub use heap::{Gc, Heap};
pub use string::LuaStr;
pub use table::{Table, TableError};
pub use value::Value;
