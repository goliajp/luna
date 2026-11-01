//! Bytecode VM: instruction set, errors, interpreter, builtins.
//!
//! The heap is the VM's own. An embedder reaches it only through the VM's
//! methods, so safe code can neither free an object the VM still uses nor
//! store one heap's objects in another heap's tables:
//!
//! ```
//! use luna_core::{runtime::Value, version::LuaVersion, vm::Vm};
//! let mut vm = Vm::new(LuaVersion::Lua54);
//! let _: Option<&luna_core::runtime::Heap> = None;
//! let t = vm.new_table().build();
//! let k = Value::Str(vm.intern("k"));
//! vm.raw_set(t, k, Value::Int(1)).unwrap();
//! ```
//!
//! Each of the following fails to compile, for the reason its comment
//! gives; the example above compiles with the same paths.
//!
//! ```compile_fail,E0616
//! use luna_core::{version::LuaVersion, vm::Vm};
//! let mut vm = Vm::new(LuaVersion::Lua54);
//! let _ = &mut vm.heap; // error[E0616]: field `heap` of struct `Vm` is private
//! ```
//!
//! ```compile_fail,E0624
//! let _ = luna_core::runtime::Heap::new(); // error[E0624]: `new` is private
//! ```
//!
//! ```compile_fail,E0624
//! use luna_core::{runtime::Value, version::LuaVersion, vm::Vm};
//! let mut vm = Vm::new(LuaVersion::Lua54);
//! let t = vm.new_table().build();
//! let k = Value::Str(vm.intern("k"));
//! t.set(unimplemented!(), k, Value::Int(1)); // error[E0624]: `set` is private
//! ```

#[macro_use]
mod rules;
pub(crate) mod argcheck;
pub mod builtins;
pub(crate) mod c_abi;
pub(crate) mod callstack;
pub(crate) mod cfmt;
pub mod dump;
pub mod error;
pub mod exec;
pub mod isa;
pub(crate) mod jit_state;
pub mod lib_bit32;
pub mod lib_coroutine;

pub mod async_drive;
pub mod ergo;
pub mod host_roots;
pub mod inspect;
pub mod into_value;
pub mod sandbox;
pub mod table_builder;
pub mod typed_native;
pub mod userdata_trait;

/// Cross-thread `SendVm` wrapper. Gated behind
/// the `send` cargo feature; embedders not opting in pay no cost.
/// See [`send_vm::SendVm`] for the design.
#[cfg(feature = "send")]
pub mod send_vm;
#[cfg(feature = "send")]
pub use send_vm::SendVm;

pub use async_drive::{AsyncNativeFn, EvalFuture};
pub use error::{LuaError, LuaErrorKind};
pub use exec::{Error, Vm};
pub use host_roots::{HostRootStale, HostRootTicket};
pub use into_value::IntoValue;
pub use sandbox::SandboxBuilder;
mod standalone;
pub use standalone::{Standalone, StandaloneOptions};
pub use table_builder::TableBuilder;
pub use typed_native::{FromLuaArgs, FromLuaValue, IntoLuaReturn, NativeTypedSig};
pub use userdata_trait::{
    LuaUserdata, MetaMethod, MetatableBuilder, UserdataMarker, UserdataMethods,
};
pub mod lib_debug;
pub(crate) mod lib_gc;
pub(crate) mod lib_io;
pub mod lib_math;
pub(crate) mod lib_mem;
pub mod lib_os_io;
pub(crate) mod lib_package;
pub(crate) mod lib_strformat;
pub mod lib_string;
pub mod lib_strpack;
pub mod lib_table;
pub mod lib_utf8;
pub mod objname;
pub(crate) mod opening;
pub(crate) mod puc_box;
pub(crate) mod puc_buffer;
