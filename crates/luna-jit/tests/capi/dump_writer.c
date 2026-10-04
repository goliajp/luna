/* how lua_dump calls the writer: the size of every call, in order, for a
   few functions, stripped and not (5.3+), and a writer that fails part way
   (the dump stops calling it and returns its status) */
#include <stdio.h>
#include <string.h>
#include "lua.h"
#include "lauxlib.h"
#include "lualib.h"

#if LUA_VERSION_NUM >= 503
#define DUMP(L, w, d, s) lua_dump(L, w, d, s)
#else
#define DUMP(L, w, d, s) lua_dump(L, w, d)
#endif

typedef struct {
  int calls;
  int fail_at;
  size_t total;
} Log;

static int writer(lua_State *L, const void *p, size_t sz, void *ud) {
  Log *g = (Log *)ud;
  (void)L;
  g->calls++;
  g->total += sz;
  printf(" %d%s", (int)sz, p == NULL ? "(null)" : "");
  return g->calls == g->fail_at ? 7 : 0;
}

static void dump(lua_State *L, const char *title, const char *src, int strip, int fail_at) {
  Log g = {0, 0, 0};
  int st;
  g.fail_at = fail_at;
  if (luaL_loadstring(L, src) != 0) {
    printf("%s: load failed: %s\n", title, lua_tostring(L, -1));
    lua_pop(L, 1);
    return;
  }
  if (strcmp(title, "inner") == 0) {
    lua_call(L, 0, 1);
  }
  printf("%s:", title);
  st = DUMP(L, writer, &g, strip);
  printf("\n  status %d, %d calls, %d bytes, top %d\n", st, g.calls, (int)g.total, lua_gettop(L));
  lua_settop(L, 0);
}

static const char *chunk =
  "local a, b = ...\n"
  "local t = {a, b, 'k', 1.5, true}\n"
  "local function f(x) return x + #t end\n"
  "return f(a) .. b\n";

int main(void) {
  lua_State *L = luaL_newstate();
  luaL_openlibs(L);
  dump(L, "empty", "", 0, 0);
  dump(L, "chunk", chunk, 0, 0);
  dump(L, "inner", "local up = 1 return function(p, q) up = up + p return q end", 0, 0);
#if LUA_VERSION_NUM >= 503
  dump(L, "chunk stripped", chunk, 1, 0);
#endif
  dump(L, "fails at call 3", chunk, 0, 3);
  dump(L, "fails at call 1", "return 1", 0, 1);
  lua_close(L);
  return 0;
}
