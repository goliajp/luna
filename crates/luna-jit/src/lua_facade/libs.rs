//! The standard libraries a [`Lua`] opens.

use super::Lua;
use luna_core::vm::LuaError;
#[cfg(doc)]
use luna_core::vm::Vm;

impl Lua {
    /// Open the base library (`print`, `type`, `pcall`, etc.).
    ///
    /// # Errors
    ///
    /// As [`Vm::open_base`].
    pub fn open_base(&mut self) -> Result<(), LuaError> {
        self.0.open_base()
    }

    /// Open the math library.
    ///
    /// # Errors
    ///
    /// As [`Vm::open_math`].
    pub fn open_math(&mut self) -> Result<(), LuaError> {
        self.0.open_math()
    }

    /// Open the string library.
    ///
    /// # Errors
    ///
    /// As [`Vm::open_string`].
    pub fn open_string(&mut self) -> Result<(), LuaError> {
        self.0.open_string()
    }

    /// Open the table library.
    ///
    /// # Errors
    ///
    /// As [`Vm::open_table`].
    pub fn open_table(&mut self) -> Result<(), LuaError> {
        self.0.open_table()
    }

    /// Open the coroutine library.
    ///
    /// # Errors
    ///
    /// As [`Vm::open_coroutine`].
    pub fn open_coroutine(&mut self) -> Result<(), LuaError> {
        self.0.open_coroutine()
    }
}
