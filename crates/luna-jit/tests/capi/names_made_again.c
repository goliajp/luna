/* the names the auxiliary library makes where a function uses them, which
   the collector frees when nothing holds them: "__metatable" (getmetatable,
   setmetatable), "__pairs" (pairs), "__tostring" and "__name" (tostring,
   and an argument's type error). The io library, whose metatable holds
   the last two, is not opened, nor any library but the basic one. Each chunk uses its name now and then while
   it makes garbage and keeps objects, so that the name dies and is made
   again where other objects lie around it; the count after every 4000
   rounds shows where the collector freed what. */
#include <stdio.h>
#include "lua.h"
#include "lauxlib.h"
#include "lualib.h"

#define LOOP(use) \
  "local t, keep, out = setmetatable({1}, {}), {}, {}\n" \
  "for i = 1, 40000 do\n" \
  "  local _ = {i}\n" \
  "  if i % 3 == 0 then keep[#keep + 1] = {} end\n" \
  "  if i % 1500 == 0 then " use " end\n" \
  "  if i % 4000 == 0 then out[#out + 1] = collectgarbage('count') * 1024 end\n" \
  "end\n" \
  "return out[1], out[2], out[3], out[4], out[5], out[6], out[7], out[8], out[9], out[10]"

static const char *const chunks[] = {
  LOOP("setmetatable(t, {}) local _ = getmetatable(t)"),
  LOOP("for _ in pairs(t) do end"),
  LOOP("local _ = #tostring(t)"),
  LOOP("pcall(select, t)"),
};

static void open(lua_State *L, const char *name, lua_CFunction f) {
#if LUA_VERSION_NUM >= 502
  luaL_requiref(L, name, f, 1);
  lua_pop(L, 1);
#else
  lua_pushcfunction(L, f);
  lua_pushstring(L, name);
  lua_call(L, 1, 0);
#endif
}

int main(void) {
  size_t i;
  lua_State *L = luaL_newstate();
  open(L, "_G", luaopen_base);
  for (i = 0; i < sizeof chunks / sizeof chunks[0]; i++) {
    if (luaL_dostring(L, chunks[i]) != 0) {
      printf("error: %s\n", lua_tostring(L, -1));
    } else {
      int k;
      for (k = 1; k <= lua_gettop(L); k++)
        printf("%.0f%s", (double)lua_tonumber(L, k), k < lua_gettop(L) ? " " : "\n");
    }
    lua_settop(L, 0);
  }
  lua_close(L);
  return 0;
}
