/* lua_getlocal and lua_setlocal: Lua locals, temporaries, varargs (5.2+),
   parameter names of a function value (5.2+), a C function's own stack,
   a suspended coroutine's locals, and what each pops or pushes */
#include <stdio.h>
#include <string.h>
#include "lua.h"
#include "lauxlib.h"
#include "lualib.h"

static void val(lua_State *L, int idx) {
  int t = lua_type(L, idx);
  if (t == LUA_TNUMBER || t == LUA_TSTRING) printf("%s", lua_tostring(L, idx));
  else if (t == LUA_TBOOLEAN) printf("%s", lua_toboolean(L, idx) ? "true" : "false");
  else printf("<%s>", lua_typename(L, t));
}

static void locals(lua_State *L, lua_State *T, int level) {
  lua_Debug ar;
  int n, top = lua_gettop(L);
  if (!lua_getstack(T, level, &ar)) { printf(" no level %d\n", level); return; }
  printf(" level %d:", level);
  for (n = -3; n <= 12; n++) {
    const char *name;
    if (n == 0) continue;
    name = lua_getlocal(T, &ar, n);
    if (name) {
      printf(" %d:%s=", n, name);
      val(T, -1);
      lua_pop(T, 1);
    }
  }
  printf(" (top %+d)\n", lua_gettop(L) - top);
}

/* list the locals of levels 0..3 of the running thread */
static int show(lua_State *L) {
  int i;
  printf("show %s\n", lua_tostring(L, 1));
  lua_pushstring(L, "c-own");
  for (i = 0; i <= 3; i++) locals(L, L, i);
  return 0;
}

/* set local n of level 1 to v; report name and stack effect */
static int set(lua_State *L) {
  lua_Debug ar;
  int n = (int)lua_tointeger(L, 1);
  int top;
  const char *name;
  lua_getstack(L, 1, &ar);
  lua_pushvalue(L, 2);
  top = lua_gettop(L);
  name = lua_setlocal(L, &ar, n);
  printf("setlocal %d -> %s (top %+d)\n", n, name ? name : "(null)", lua_gettop(L) - top);
  lua_settop(L, 0);
  return 0;
}

/* set a slot of this C function's own stack through the debug interface */
static int cself(lua_State *L) {
  lua_Debug ar;
  const char *name;
  lua_settop(L, 0);
  lua_pushinteger(L, 10);
  lua_pushinteger(L, 20);
  lua_getstack(L, 0, &ar);
  lua_pushstring(L, "new");
  name = lua_setlocal(L, &ar, 2);
  printf("cself setlocal -> %s top=%d slot2=", name ? name : "(null)", lua_gettop(L));
  val(L, 2);
  printf("\n");
  name = lua_getlocal(L, &ar, 3);
  printf("cself getlocal 3 -> %s\n", name ? name : "(null)");
  if (name) lua_pop(L, 1);
  name = lua_getlocal(L, &ar, 0);
  printf("cself getlocal 0 -> %s\n", name ? name : "(null)");
  return 0;
}

static int params(lua_State *L) {
#if LUA_VERSION_NUM >= 502
  int n, top = lua_gettop(L);
  printf("params:");
  for (n = 0; n <= 4; n++) {
    const char *name = lua_getlocal(L, NULL, n);
    printf(" %d=%s", n, name ? name : "(null)");
  }
  printf(" (top %+d)\n", lua_gettop(L) - top);
#else
  (void)L;
#endif
  return 0;
}

static int coshow(lua_State *L) {
  lua_State *co = lua_tothread(L, 1);
  lua_Debug ar;
  printf("coroutine\n");
  locals(L, co, 0);
  locals(L, co, 1);
  if (lua_getstack(co, 1, &ar)) {
    const char *name;
    lua_pushstring(co, "changed");
    name = lua_setlocal(co, &ar, 1);
    printf(" co setlocal -> %s\n", name ? name : "(null)");
  }
  return 0;
}

static const char *script =
  "local show, set, cself, params, coshow = ...\n"
  "local function f(a, b, ...)\n"
  "  local x = 'xv'\n"
  "  do local inner = 1 end\n"
  "  show('f')\n"
  "  set(1, 'A') set(3, 'X') set(40, 'none')\n"
  "  if select('#', ...) > 0 then set(-1, 'V') end\n"
  "  return a, x, ...\n"
  "end\n"
  "print(f(1, 2, 'va1', 'va2'))\n"
  "print(f(1))\n"
  "local t = setmetatable({}, {__index = function(t, k) show('mm') end})\n"
  "local _ = t.k\n"
  "cself()\n"
  "params(f)\n"
  "params(print)\n"
  "params(function(p, q) local r end)\n"
  "local co = coroutine.create(function(p)\n"
  "  local q = p .. '!'\n"
  "  coroutine.yield(q)\n"
  "  print('after yield', p, q)\n"
  "end)\n"
  "coroutine.resume(co, 'arg')\n"
  "coshow(co)\n"
  "coroutine.resume(co)\n"
  "pcall(show, 'pcall')\n";

int main(void) {
  lua_State *L = luaL_newstate();
  int st;
  luaL_openlibs(L);
  st = luaL_loadstring(L, script);
  lua_pushcfunction(L, show);
  lua_pushcfunction(L, set);
  lua_pushcfunction(L, cself);
  lua_pushcfunction(L, params);
  lua_pushcfunction(L, coshow);
  if (st == 0) st = lua_pcall(L, 5, 0, 0);
  printf("script: %d %s\n", st, st ? lua_tostring(L, -1) : "ok");
  lua_close(L);
  return 0;
}
