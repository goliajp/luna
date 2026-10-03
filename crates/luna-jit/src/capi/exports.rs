//! The exported names of the API functions written in C (`csrc/`). A
//! shared library Rust builds exports only symbols defined in Rust, so
//! each of these names is a Rust function with no frame of its own that
//! jumps to the C function: the call arrives there as if made directly,
//! arguments and return address untouched, and an error thrown from it
//! crosses no Rust frame.
//!
//! They are not meant to be called from Rust: their Rust signatures are
//! empty. The headers in `include/` declare them for C. A module that adds
//! C functions lists them with `c_exports!` (this module is
//! `#[macro_use]`).

#[cfg(any(target_arch = "x86_64", target_arch = "x86"))]
macro_rules! jump {
    ($t:ident) => {
        core::arch::naked_asm!("jmp {f}", f = sym $t)
    };
}

#[cfg(any(target_arch = "aarch64", target_arch = "arm"))]
macro_rules! jump {
    ($t:ident) => {
        core::arch::naked_asm!("b {f}", f = sym $t)
    };
}

#[cfg(any(target_arch = "riscv64", target_arch = "riscv32"))]
macro_rules! jump {
    ($t:ident) => {
        core::arch::naked_asm!("tail {f}", f = sym $t)
    };
}

#[cfg(target_arch = "s390x")]
macro_rules! jump {
    ($t:ident) => {
        core::arch::naked_asm!("jg {f}", f = sym $t)
    };
}

macro_rules! c_exports {
    ($($name:ident => $target:ident),* $(,)?) => {
        // SAFETY: each name is a C function defined in `csrc/`; Rust only
        // takes its address for the jumps below and never calls it
        unsafe extern "C" {
            $(fn $target();)*
        }
        $(
            #[unsafe(naked)]
            // SAFETY: no other item in the link has this name: the host does
            // not link PUC's liblua next to this crate, which defines each
            // `lua_*` name once
            #[unsafe(no_mangle)]
            pub extern "C" fn $name() {
                jump!($target)
            }
        )*
    };
}

// the ones the core of the C API defines; each family of functions lists
// its own next to its Rust side
c_exports! {
    lua_error => luna_c_lua_error,
    lua_callk => luna_c_lua_callk,
    luna_callk_52 => luna_c_luna_callk_52,
    lua_call => luna_c_lua_call,
    lua_pcallk => luna_c_lua_pcallk,
    luna_pcallk_52 => luna_c_luna_pcallk_52,
    lua_yieldk => luna_c_lua_yieldk,
    luna_yieldk_52 => luna_c_luna_yieldk_52,
    lua_yield => luna_c_lua_yield,
    lua_settop => luna_c_lua_settop,
    lua_pop => luna_c_lua_pop,
    lua_toclose => luna_c_lua_toclose,
    lua_closeslot => luna_c_lua_closeslot,
    lua_getglobal => luna_c_lua_getglobal,
    lua_setglobal => luna_c_lua_setglobal,
    lua_register => luna_c_lua_register,
}
