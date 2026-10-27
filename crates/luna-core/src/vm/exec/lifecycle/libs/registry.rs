//! The keys PUC's registry has that are addresses.

use super::*;

/// The light userdata PUC keys some registry entries by: 5.3's `CLIBS`,
/// and the debug library's hook table in 5.1 and 5.3. PUC's are the
/// addresses of statics of its own; luna's are fixed values below any page
/// a system maps, never dereferenced, so that where they sit in the
/// registry does not move with the build. The PUC the counts are compared
/// with without coincidences of addresses takes the same values.
pub(crate) const CLIBS_KEY: usize = 0x1010;
pub(crate) const HOOK_KEY: usize = 0x1020;

/// The light userdata of the fixed value `k`.
pub(crate) fn light_key(k: usize) -> Value {
    Value::LightUserdata(std::ptr::without_provenance(k))
}
