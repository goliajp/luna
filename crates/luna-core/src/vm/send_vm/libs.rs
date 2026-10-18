//! The standard libraries a `SendVm` installs, each through the wrapped Vm.

use super::SendVm;
use crate::vm::error::LuaError;
#[cfg(doc)]
use crate::vm::exec::Vm;

impl SendVm {
    /// Install the base library on this `SendVm`. Mirror of
    /// [`Vm::open_base`].
    ///
    /// # Errors
    ///
    /// As [`Vm::open_base`].
    pub fn open_base(&self) -> Result<(), LuaError> {
        self.with_vm_mut(|vm| vm.open_base())
    }

    /// Install the math library.
    ///
    /// # Errors
    ///
    /// As [`Vm::open_math`].
    pub fn open_math(&self) -> Result<(), LuaError> {
        self.with_vm_mut(|vm| vm.open_math())
    }

    /// Install the string library.
    ///
    /// # Errors
    ///
    /// As [`Vm::open_string`].
    pub fn open_string(&self) -> Result<(), LuaError> {
        self.with_vm_mut(|vm| vm.open_string())
    }

    /// Install the table library.
    ///
    /// # Errors
    ///
    /// As [`Vm::open_table`].
    pub fn open_table(&self) -> Result<(), LuaError> {
        self.with_vm_mut(|vm| vm.open_table())
    }

    /// Install the coroutine library.
    ///
    /// # Errors
    ///
    /// As [`Vm::open_coroutine`].
    pub fn open_coroutine(&self) -> Result<(), LuaError> {
        self.with_vm_mut(|vm| vm.open_coroutine())
    }
}
