//! C hosts of the C API: values (see `capi_hosts`).

use super::capi_hosts::check;

#[test]
fn values_ops() {
    check("values_ops");
}

#[test]
fn values_meta() {
    check("values_meta");
}

#[test]
fn values_userdata() {
    check("values_userdata");
}
