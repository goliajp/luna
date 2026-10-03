/*
 * The debug interface's C side: the debug library's hook (PUC hookf),
 * which calls a thread's Lua hook and throws what it raises.
 */
#include "shim.h"

void luna_capi_hookf(lua_State *L, lua_Debug *ar);

LUNA_HIDDEN void luna_c_hookf(lua_State *L, lua_Debug *ar) {
  luna_capi_hookf(L, ar);
  luna_check(L);
}
