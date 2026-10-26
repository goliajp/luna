/* PUC's stack holds what C pushed below a call: a deep call from a host
   that has values on its stack, or from a C function that pushed some,
   grows the stack sooner, and the count shows the stack's size. For
   depths around where the stack grows, the count after a call with 0, 1
   and 9 values below it, from the host and from a C function; and after
   a deep call that fails with 0 and 30 values below it, the stack shrunk
   to what is in use, those values counted. */
#include <stdio.h>
#include "lua.h"
#include "lauxlib.h"
#include "lualib.h"

static const char *src =
    "local function deep(n) if n > 0 then return deep(n - 1) + 1 end return 0 end\n"
    "local below, from_c, n, fail = ...\n"
    "if from_c then return pushing(below, deep, n) end\n"
    "if fail then deep(n) error('fail') end\n"
    "return deep(n)\n";

/* pushing(k, f, n): k values pushed, then f(n) */
static int pushing(lua_State *L) {
  int k = (int)lua_tointeger(L, 1), i;
  lua_checkstack(L, k + 2);
  for (i = 0; i < k; i++) lua_pushnil(L);
  lua_pushvalue(L, 2);
  lua_pushvalue(L, 3);
  lua_call(L, 1, 1);
  return 1;
}

static long count(lua_State *L) {
  return (long)lua_gc(L, LUA_GCCOUNT, 0) * 1024 + lua_gc(L, LUA_GCCOUNTB, 0);
}

static long run(int below, int from_c, int n, int fail) {
  lua_State *L = luaL_newstate();
  long before;
  int i;
  lua_gc(L, LUA_GCSTOP, 0);
  luaL_openlibs(L);
  lua_register(L, "pushing", pushing);
  luaL_loadstring(L, src);
  lua_checkstack(L, below + 8);
  if (!from_c) {
    /* the values go below the function */
    for (i = 0; i < below; i++) {
      lua_pushnil(L);
      lua_insert(L, -2);
    }
  }
  lua_pushinteger(L, below);
  lua_pushboolean(L, from_c);
  lua_pushinteger(L, n);
  lua_pushboolean(L, fail);
  before = count(L);
  if (lua_pcall(L, 4, 0, 0) != 0 && !fail) printf("error: %s\n", lua_tostring(L, -1));
  before = count(L) - before;
  lua_close(L);
  return before;
}

int main(void) {
  static const int belows[] = {0, 1, 9};
  int b, from_c, n;
  for (from_c = 0; from_c <= 1; from_c++)
    for (b = 0; b < 3; b++) {
      printf("%s %d:", from_c ? "c" : "host", belows[b]);
      for (n = 0; n <= 24; n++) printf(" %ld", run(belows[b], from_c, n, 0));
      printf("\n");
    }
  for (b = 0; b <= 30; b += 30) {
    printf("fail %d:", b);
    for (n = 0; n <= 60; n += 4) printf(" %ld", run(b, 0, n, 1));
    printf("\n");
  }
  return 0;
}
