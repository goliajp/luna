/* lua_getupvalue, lua_setupvalue, lua_upvalueid and lua_upvaluejoin on Lua
   and C closures, stripped functions, and indices out of range */
#include <stdio.h>
#include <string.h>
#include "lua.h"
#include "lauxlib.h"
#include "lualib.h"

static void val(lua_State *L, int idx) {
  int t = lua_type(L, idx);
  if (t == LUA_TNUMBER || t == LUA_TSTRING) printf("%s", lua_tostring(L, idx));
  else printf("<%s>", lua_typename(L, t));
}

static void ups(lua_State *L, const char *what) {
  int n;
  printf("%s:", what);
  for (n = -1; n <= 4; n++) {
    int top = lua_gettop(L);
    const char *name = lua_getupvalue(L, -1, n);
    if (name) {
      printf(" %d:[%s]=", n, name);
      val(L, -1);
      lua_pop(L, 1);
    } else if (lua_gettop(L) != top) printf(" pushed on NULL!");
  }
  printf("\n");
}

static int cfn(lua_State *L) {
  lua_pushvalue(L, lua_upvalueindex(1));
  return 1;
}

static void setup(lua_State *L, const char *name, int n, const char *v) {
  int top = lua_gettop(L);
  const char *r;
  lua_pushstring(L, v);
  r = lua_setupvalue(L, -2, n);
  printf("setupvalue %s %d -> %s (top %+d)\n", name, n, r ? r : "(null)", lua_gettop(L) - top);
  lua_settop(L, top);
}

int main(void) {
  lua_State *L = luaL_newstate();
  luaL_openlibs(L);
  luaL_loadstring(L,
    "local a, b = 1, 2\n"
    "local function f() return a + b end\n"
    "local function g() return b end\n"
    "local function h() return a end\n"
    "F, G, H = f, g, h\n"
    "if string.dump then\n"
    "  local ok, s = pcall(string.dump, f, true)\n"
    "  if ok then STRIPPED = (loadstring or load)(s) end\n"
    "end\n");
  lua_call(L, 0, 0);
  lua_getglobal(L, "F");
  ups(L, "F");
  setup(L, "F", 1, "10");
  setup(L, "F", 3, "x");
  luaL_loadstring(L, "return F()");
  lua_call(L, 0, 1);
  printf("F() = %s\n", lua_tostring(L, -1));
  lua_settop(L, 0);
  lua_getglobal(L, "STRIPPED");
  if (!lua_isnil(L, -1)) {
    /* only the names: loading gives the first upvalue the globals */
    int n;
    printf("stripped:");
    for (n = 1; n <= 3; n++) {
      const char *name = lua_getupvalue(L, -1, n);
      if (name) { printf(" %d:[%s]", n, name); lua_pop(L, 1); }
    }
    printf("\n");
  }
  lua_settop(L, 0);
  lua_pushstring(L, "u1");
  lua_pushinteger(L, 2);
  lua_pushcclosure(L, cfn, 2);
  ups(L, "cclosure");
  setup(L, "C", 1, "u1new");
  setup(L, "C", 3, "none");
  lua_call(L, 0, 1);
  printf("cfn() = %s\n", lua_tostring(L, -1));
  lua_settop(L, 0);
  lua_pushcfunction(L, cfn);
  ups(L, "lightc");
  lua_pushinteger(L, 5);
  ups(L, "number");
  lua_settop(L, 0);
#if LUA_VERSION_NUM >= 502
  {
    void *fa, *fb, *gb, *ha;
    lua_getglobal(L, "F");
    lua_getglobal(L, "G");
    lua_getglobal(L, "H");
    fa = lua_upvalueid(L, 1, 1);
    fb = lua_upvalueid(L, 1, 2);
    gb = lua_upvalueid(L, 2, 1);
    ha = lua_upvalueid(L, 3, 1);
    printf("ids: fa==fb %d fb==gb %d fa==ha %d fa null %d\n", fa == fb, fb == gb, fa == ha, fa == NULL);
    lua_upvaluejoin(L, 2, 1, 3, 1);
    printf("joined: g1==h1 %d\n", lua_upvalueid(L, 2, 1) == lua_upvalueid(L, 3, 1));
    luaL_loadstring(L, "return G()");
    lua_call(L, 0, 1);
    printf("G() = %s\n", lua_tostring(L, -1));
    lua_settop(L, 0);
    lua_pushinteger(L, 1);
    lua_pushinteger(L, 2);
    lua_pushcclosure(L, cfn, 2);
    printf("c ids: 1==2 %d 1==1 %d null %d\n", lua_upvalueid(L, -1, 1) == lua_upvalueid(L, -1, 2),
           lua_upvalueid(L, -1, 1) == lua_upvalueid(L, -1, 1), lua_upvalueid(L, -1, 1) == NULL);
#if LUA_VERSION_NUM >= 504
    printf("out of range: %d %d\n", lua_upvalueid(L, -1, 3) == NULL, lua_upvalueid(L, -1, 0) == NULL);
    lua_getglobal(L, "F");
    printf("lua out of range: %d\n", lua_upvalueid(L, -1, 3) == NULL);
#endif
  }
#endif
  lua_close(L);
  return 0;
}
