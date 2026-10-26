/* a loop the trace compiler compiles makes garbage tables and overwrites
   kept ones: the count after each round and after a full collection is
   PUC's (a table a compiled loop made for real, counted as PUC's from
   then on, takes its bytes off the count when it is freed) */
#include <stdio.h>
#include "lua.h"
#include "lauxlib.h"
#include "lualib.h"

static const char *const chunk =
  "local keep, out = {}, {}\n"
  "for r = 1, 3 do\n"
  "  for i = 1, 200000 do\n"
  "    local _ = {i}\n"
  "    if i % 3 == 0 then keep[#keep % 1000 + 1] = {} end\n"
  "  end\n"
  "  out[#out + 1] = collectgarbage('count') * 1024\n"
  "  collectgarbage()\n"
  "  out[#out + 1] = collectgarbage('count') * 1024\n"
  "end\n"
  "return out[1], out[2], out[3], out[4], out[5], out[6]";

int main(void) {
  int k;
  lua_State *L = luaL_newstate();
#if LUA_VERSION_NUM >= 502
  luaL_requiref(L, "_G", luaopen_base, 1);
  lua_pop(L, 1);
#else
  lua_pushcfunction(L, luaopen_base);
  lua_pushstring(L, "");
  lua_call(L, 1, 0);
#endif
  if (luaL_dostring(L, chunk) != 0) {
    printf("error: %s\n", lua_tostring(L, -1));
  } else {
    for (k = 1; k <= lua_gettop(L); k++)
      printf("%.0f%s", (double)lua_tonumber(L, k), k < lua_gettop(L) ? " " : "\n");
  }
  lua_close(L);
  return 0;
}
