//! Minimal base library. The full base library replaces/extends this.

use crate::runtime::Value;
use crate::version::LuaVersion;
use crate::vm::argcheck::{self, Args};
use crate::vm::error::LuaError;
use crate::vm::exec::Vm;
mod puc_fns;
pub(crate) use puc_fns::PUC_FNS;

mod arg_error;
mod basic;
mod compat51;
mod errors;
mod iter;
mod load;
mod protected;
mod tonumber;
mod tostring;

pub(crate) use arg_error::arg_error;
use basic::{
    nat_getmetatable, nat_print, nat_rawequal, nat_rawget, nat_rawlen, nat_rawset, nat_select,
    nat_setmetatable, nat_type, nat_warn,
};
use compat51::{nat_gcinfo, nat_getfenv, nat_newproxy, nat_setfenv};
use errors::nat_assert;
pub(crate) use errors::nat_error;
pub(crate) use errors::{concat2, lual_where, raise_bytes, raise_fmt, raise_lib, raise_str};
pub(crate) use iter::ipairs_iter;
use iter::{nat_ipairs, nat_next};
pub(crate) use iter::{nat_pairs, pairs_mm_results};
pub(crate) use load::nat_load;
use load::nat_loadstring;
pub(crate) use protected::{
    nat_host_pcall, nat_host_pcall_in_c, nat_host_xpcall, nat_host_xpcall_in_c, nat_pcall,
    nat_xpcall, xpcall_handler,
};
pub(crate) use tonumber::nat_tonumber;
use tostring::nat_tostring;
