//! C hosts of the C API: debug (see `capi_hosts`).

use super::capi_hosts::check;

#[test]
fn debug_info() {
    check("debug_info")
}
