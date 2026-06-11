//! Runtime core (P02): values, GC heap, strings, tables.

pub mod heap;
pub mod string;
pub mod table;
pub mod value;

pub use heap::{Gc, Heap};
pub use string::LuaStr;
pub use table::{Table, TableError};
pub use value::Value;
