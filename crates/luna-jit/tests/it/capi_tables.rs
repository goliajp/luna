//! C hosts of the C API: tables (see `capi_hosts`).

use super::capi_hosts::check;

#[test]
fn tables_get() {
    check("tables_get");
}

#[test]
fn tables_set() {
    check("tables_set");
}
