//! Return encoding: `IntoLuaReturn` for single values and tuples.

use crate::runtime::value::Value;
use crate::vm::error::LuaError;
use crate::vm::exec::Vm;
use crate::vm::into_value::IntoValue;

/// Push a typed Rust value (or tuple of values) onto the VM's stack as a
/// native function's return values.
pub trait IntoLuaReturn {
    /// Push the encoded values starting at `fs` and return the result count.
    fn into_lua_return(self, vm: &mut Vm, fs: u32) -> Result<u32, LuaError>;
}

impl IntoLuaReturn for () {
    #[inline]
    fn into_lua_return(self, vm: &mut Vm, fs: u32) -> Result<u32, LuaError> {
        Ok(vm.nat_return(fs, &[]))
    }
}

impl<Out: IntoLuaReturn> IntoLuaReturn for Result<Out, LuaError> {
    #[inline]
    fn into_lua_return(self, vm: &mut Vm, fs: u32) -> Result<u32, LuaError> {
        self?.into_lua_return(vm, fs)
    }
}

macro_rules! impl_into_lua_return_single {
    ($($t:ty),+ $(,)?) => {
        $(
            impl IntoLuaReturn for $t {
                #[inline]
                fn into_lua_return(self, vm: &mut Vm, fs: u32) -> Result<u32, LuaError> {
                    let v = <$t as IntoValue>::into_value(self, vm);
                    Ok(vm.nat_return(fs, &[v]))
                }
            }
        )+
    };
}
impl_into_lua_return_single!(
    Value,
    i64,
    i32,
    i16,
    i8,
    u32,
    u16,
    u8,
    f64,
    f32,
    bool,
    String,
    Vec<u8>,
);

impl IntoLuaReturn for &'static str {
    #[inline]
    fn into_lua_return(self, vm: &mut Vm, fs: u32) -> Result<u32, LuaError> {
        let v = self.into_value(vm);
        Ok(vm.nat_return(fs, &[v]))
    }
}

macro_rules! impl_into_lua_return_tuple {
    ( $( ($($name:ident: $idx:tt),+) ),+ $(,)? ) => {
        $(
            impl<$($name: IntoValue),+> IntoLuaReturn for ($($name,)+) {
                #[inline]
                fn into_lua_return(self, vm: &mut Vm, fs: u32) -> Result<u32, LuaError> {
                    let vs = [
                        $( self.$idx.into_value(vm), )+
                    ];
                    Ok(vm.nat_return(fs, &vs))
                }
            }
        )+
    };
}
impl_into_lua_return_tuple! {
    (T0: 0, T1: 1),
    (T0: 0, T1: 1, T2: 2),
    (T0: 0, T1: 1, T2: 2, T3: 3),
    (T0: 0, T1: 1, T2: 2, T3: 3, T4: 4),
    (T0: 0, T1: 1, T2: 2, T3: 3, T4: 4, T5: 5),
}
