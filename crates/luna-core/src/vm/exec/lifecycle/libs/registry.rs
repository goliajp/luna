//! The keys PUC's registry has that are addresses.

use super::*;

/// The addresses PUC keys some registry entries by (light userdata): 5.3's
/// `CLIBS`, and the debug library's hook table in 5.1 and 5.3.
pub(crate) static CLIBS_KEY: u8 = 0;
pub(crate) static HOOK_KEY: u8 = 0;

/// A light userdata keyed by the address of `k`.
pub(crate) fn light_key(k: &'static u8) -> Value {
    Value::LightUserdata(k as *const u8 as *const ())
}
