//! Index and newindex done by a native for the C function it stands for:
//! what that function has pushed counts under a metamethod or an error.

use super::*;

impl Vm {
    /// [`Vm::index_value_pushed`] with the values the native pushes for
    /// it, `held`, written where they go when something can see them.
    pub(crate) fn index_value_holding(
        &mut self,
        t: Value,
        key: Value,
        held: &[Value],
    ) -> Result<Value, LuaError> {
        match self.index_step(t, key) {
            Ok(MmOut::Done(v)) => Ok(v),
            Ok(MmOut::Mm { func, recv }) => {
                self.native_push_vals(held)?;
                let r = self.call_mm1(func, &[recv, key])?;
                self.native_pop(held.len() as u32);
                Ok(r)
            }
            Ok(MmOut::CompareSynth { .. }) => unreachable!("CompareSynth from index_step"),
            Err(e) => {
                self.native_push_vals(held)?;
                Err(e)
            }
        }
    }

    /// [`Vm::index_value`] from a native that has `extra` values pushed
    /// where PUC's C function would have them by now (a key, results so
    /// far, a buffer's slot): an `__index` it runs starts above them, and
    /// an error it raises counts them. Nothing is counted on the raw path.
    pub(crate) fn index_value_pushed(
        &mut self,
        t: Value,
        key: Value,
        extra: u32,
    ) -> Result<Value, LuaError> {
        match self.index_step(t, key) {
            Ok(MmOut::Done(v)) => Ok(v),
            Ok(MmOut::Mm { func, recv }) => {
                self.native_push(extra);
                let r = self.call_mm1(func, &[recv, key])?;
                self.native_pop(extra);
                Ok(r)
            }
            Ok(MmOut::CompareSynth { .. }) => unreachable!("CompareSynth from index_step"),
            Err(e) => {
                self.native_push(extra);
                Err(e)
            }
        }
    }

    /// [`Vm::newindex_value_pushed`] with the values the native pushes for
    /// it, `held`, written where they go when something can see them.
    pub(crate) fn newindex_value_holding(
        &mut self,
        t: Value,
        key: Value,
        v: Value,
        held: &[Value],
    ) -> Result<(), LuaError> {
        match self.newindex_step(t, key, v) {
            Ok(MmOut::Done(_)) => Ok(()),
            Ok(MmOut::Mm { func, recv }) => {
                self.native_push_vals(held)?;
                self.call_value(func, &[recv, key, v])?;
                self.native_pop(held.len() as u32);
                Ok(())
            }
            Ok(MmOut::CompareSynth { .. }) => unreachable!("CompareSynth from newindex_step"),
            Err(e) => {
                self.native_push_vals(held)?;
                Err(e)
            }
        }
    }

    /// [`Vm::newindex_value`] with `extra` values pushed, as
    /// [`Vm::index_value_pushed`].
    pub(crate) fn newindex_value_pushed(
        &mut self,
        t: Value,
        key: Value,
        v: Value,
        extra: u32,
    ) -> Result<(), LuaError> {
        match self.newindex_step(t, key, v) {
            Ok(MmOut::Done(_)) => Ok(()),
            Ok(MmOut::Mm { func, recv }) => {
                self.native_push(extra);
                self.call_value(func, &[recv, key, v])?;
                self.native_pop(extra);
                Ok(())
            }
            Ok(MmOut::CompareSynth { .. }) => unreachable!("CompareSynth from newindex_step"),
            Err(e) => {
                self.native_push(extra);
                Err(e)
            }
        }
    }
}
