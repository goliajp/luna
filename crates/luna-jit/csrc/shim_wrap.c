/*
 * The API functions that may raise an error or yield: each calls its Rust
 * side, which reports an error or a yield through the state's `raised`
 * field after it has returned, and throws it from here.
 */
#include "shim.h"

#define WRAP_V(name, P, A)                    \
  void luna_capi_##name P;                    \
  LUNA_HIDDEN void luna_c_##name P {          \
    luna_capi_##name A;                       \
    luna_check(L);                            \
  }

#define WRAP_R(R, name, P, A)                 \
  R luna_capi_##name P;                       \
  LUNA_HIDDEN R luna_c_##name P {             \
    R r_ = luna_capi_##name A;                \
    luna_check(L);                            \
    return r_;                                \
  }

WRAP_V(lua_callk, (lua_State *L, int nargs, int nresults, lua_KContext ctx, lua_KFunction k),
       (L, nargs, nresults, ctx, k))
WRAP_V(luna_callk_52, (lua_State *L, int nargs, int nresults, int ctx, lua_CFunction k),
       (L, nargs, nresults, ctx, k))
WRAP_V(lua_call, (lua_State *L, int nargs, int nresults), (L, nargs, nresults))
WRAP_R(int, lua_pcallk,
       (lua_State *L, int nargs, int nresults, int msgh, lua_KContext ctx, lua_KFunction k),
       (L, nargs, nresults, msgh, ctx, k))
WRAP_R(int, luna_pcallk_52,
       (lua_State *L, int nargs, int nresults, int msgh, int ctx, lua_CFunction k),
       (L, nargs, nresults, msgh, ctx, k))
WRAP_R(int, lua_yieldk, (lua_State *L, int nresults, lua_KContext ctx, lua_KFunction k),
       (L, nresults, ctx, k))
WRAP_R(int, luna_yieldk_52, (lua_State *L, int nresults, int ctx, lua_CFunction k),
       (L, nresults, ctx, k))
WRAP_R(int, lua_yield, (lua_State *L, int nresults), (L, nresults))
WRAP_V(lua_settop, (lua_State *L, int idx), (L, idx))
WRAP_V(lua_pop, (lua_State *L, int n), (L, n))
WRAP_V(lua_toclose, (lua_State *L, int idx), (L, idx))
WRAP_V(lua_closeslot, (lua_State *L, int idx), (L, idx))
WRAP_R(int, lua_getglobal, (lua_State *L, const char *name), (L, name))
WRAP_V(lua_setglobal, (lua_State *L, const char *name), (L, name))
WRAP_V(lua_register, (lua_State *L, const char *name, lua_CFunction f), (L, name, f))
