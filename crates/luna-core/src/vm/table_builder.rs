//! `TableBuilder` + `vm.table_of` (B3, Phase 2 P2-B) — replace the
//! dogfood §4.1 `unsafe { t.as_mut() }.set(...)` dance with a safe
//! one-line builder.
//!
//! ```
//! use luna_core::vm::Vm;
//! use luna_core::version::LuaVersion;
//! use luna_core::runtime::Value;
//!
//! let mut vm = Vm::sandbox(LuaVersion::Lua55).open_base().build();
//!
//! // One-shot: table_of
//! let t = vm.table_of([("answer", 42_i64), ("year", 2026)]);
//! vm.set_global("constants", Value::Table(t)).unwrap();
//!
//! // Multi-step: new_table builder
//! let t = vm.new_table()
//!     .with("name", "luna")
//!     .with("major", 1_i64)
//!     .with("minor", 1_i64)
//!     .build();
//! vm.set_global("info", Value::Table(t)).unwrap();
//!
//! let r = vm.eval("return constants.answer + info.minor").unwrap();
//! assert_eq!(r.len(), 1);
//! ```
//!
//! The unsafe `Gc::as_mut` lives inside the builder; embedders never
//! write it.

use crate::runtime::Table;
use crate::runtime::heap::Gc;
use crate::vm::error::LuaError;
use crate::vm::exec::Vm;
use crate::vm::into_value::IntoValue;

/// Multi-step table construction. Borrows `&mut Vm` for the whole
/// builder window so no other Vm operation can interleave (which
/// might trigger GC mid-build). Consume with [`TableBuilder::build`].
pub struct TableBuilder<'vm> {
    vm: &'vm mut Vm,
    t: Gc<Table>,
}

impl<'vm> TableBuilder<'vm> {
    /// Add a `(key, value)` entry. Both may be any [`IntoValue`].
    /// Panics if the table overflows (`MAX_ASIZE = 1<<27`; unreachable
    /// in practice — embedders building tables that large have other
    /// problems).
    pub fn with<K, V>(self, k: K, v: V) -> Self
    where
        K: IntoValue,
        V: IntoValue,
    {
        let TableBuilder { vm, t } = self;
        let k = k.into_value(vm);
        let v = v.into_value(vm);
        // SAFETY: Gc<T> is NonNull<T> over the single-threaded GC heap
        // (see heap.rs:5-7); the TableBuilder's exclusive &mut Vm borrow
        // guarantees no concurrent access to the table.
        unsafe { t.as_mut() }
            .set(&mut vm.heap, k, v)
            .expect("table builder overflow");
        TableBuilder { vm, t }
    }

    /// Fallible variant of [`TableBuilder::with`] — propagates
    /// `TableError::Overflow` as `LuaError` instead of panicking.
    pub fn try_with<K, V>(self, k: K, v: V) -> Result<Self, LuaError>
    where
        K: IntoValue,
        V: IntoValue,
    {
        let TableBuilder { vm, t } = self;
        let k = k.into_value(vm);
        let v = v.into_value(vm);
        // SAFETY: same as with().
        unsafe { t.as_mut() }.set(&mut vm.heap, k, v)?;
        Ok(TableBuilder { vm, t })
    }

    /// Finalize: emit a GC write barrier (so any newly-rooted children
    /// are visible to the collector) and return the table handle.
    pub fn build(self) -> Gc<Table> {
        let TableBuilder { vm, t } = self;
        vm.heap
            .barrier_back(t.as_ptr() as *mut crate::runtime::heap::GcHeader);
        t
    }
}

impl Vm {
    /// Allocate a fresh table and return a builder for in-place population.
    pub fn new_table(&mut self) -> TableBuilder<'_> {
        let t = self.heap.new_table();
        TableBuilder { vm: self, t }
    }

    /// Allocate a fresh table populated from a fixed-size slice of
    /// `(key, value)` pairs. Equivalent to chained `new_table().with(...)`
    /// calls, but more concise for static tables (stdlib registration,
    /// embedder-side constants).
    ///
    /// Panics on table overflow (unreachable for `N` small).
    pub fn table_of<K, V, const N: usize>(&mut self, entries: [(K, V); N]) -> Gc<Table>
    where
        K: IntoValue,
        V: IntoValue,
    {
        let mut b = self.new_table();
        for (k, v) in entries {
            b = b.with(k, v);
        }
        b.build()
    }
}
