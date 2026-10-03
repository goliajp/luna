/*
 * Tables, metatables and operations with metamethods: the API functions
 * of this family that may raise an error.
 */
#include "shim.h"

WRAP_R(int, lua_gettable, (lua_State *L, int idx), (L, idx))
WRAP_R(int, lua_getfield, (lua_State *L, int idx, const char *k), (L, idx, k))
WRAP_R(int, lua_geti, (lua_State *L, int idx, lua_Integer n), (L, idx, n))
WRAP_V(lua_settable, (lua_State *L, int idx), (L, idx))
WRAP_V(lua_setfield, (lua_State *L, int idx, const char *k), (L, idx, k))
WRAP_V(lua_seti, (lua_State *L, int idx, lua_Integer n), (L, idx, n))
WRAP_V(lua_rawset, (lua_State *L, int idx), (L, idx))
WRAP_V(lua_rawseti, (lua_State *L, int idx, lua_Integer n), (L, idx, n))
WRAP_V(luna_rawseti_51, (lua_State *L, int idx, int n), (L, idx, n))
WRAP_V(lua_rawsetp, (lua_State *L, int idx, const void *p), (L, idx, p))
WRAP_R(int, lua_next, (lua_State *L, int idx), (L, idx))
WRAP_R(int, lua_setmetatable, (lua_State *L, int idx), (L, idx))
WRAP_V(lua_arith, (lua_State *L, int op), (L, op))
WRAP_R(int, lua_compare, (lua_State *L, int i1, int i2, int op), (L, i1, i2, op))
WRAP_R(int, lua_equal, (lua_State *L, int i1, int i2), (L, i1, i2))
WRAP_R(int, lua_lessthan, (lua_State *L, int i1, int i2), (L, i1, i2))
WRAP_V(lua_len, (lua_State *L, int idx), (L, idx))

/* its Rust side is shim.h's luna_capi_concat, which C callers share */
LUNA_HIDDEN void luna_c_lua_concat(lua_State *L, int n) {
  luna_capi_concat(L, n);
  luna_check(L);
}
