/* lua_getlocal on the level of `load` while it calls a reader: the
   reserved slots, the piece kept, and what the parser or the undumper
   holds there (tables by their number of entries, a string by its
   length), for a text chunk and for a binary one */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "lua.h"
#include "lauxlib.h"
#include "lualib.h"

static size_t entries(lua_State *L, int idx) {
  size_t n = 0;
  lua_pushnil(L);
  while (lua_next(L, idx)) { n++; lua_pop(L, 1); }
  return n;
}

/* reader(src, step): the next `step` bytes of `src` (upvalues), listing
   the locals of level 1 first */
static int reader(lua_State *L) {
  lua_Debug ar;
  size_t len, step = (size_t)lua_tointeger(L, lua_upvalueindex(2));
  const char *src = lua_tolstring(L, lua_upvalueindex(1), &len);
  size_t at = (size_t)lua_tointeger(L, lua_upvalueindex(3));
  int n;
  if (lua_getstack(L, 1, &ar)) {
    printf("read %d:", (int)(at / step));
    for (n = 1; n <= 30; n++) {
      const char *name = lua_getlocal(L, &ar, n);
      int t;
      if (!name) break;
      t = lua_type(L, -1);
      if (t == LUA_TTABLE) printf(" table#%d", (int)entries(L, lua_gettop(L)));
      else if (t == LUA_TSTRING) {
        size_t l;
        lua_tolstring(L, -1, &l);
        printf(" string#%d", (int)l);
      }
      else printf(" %s", lua_typename(L, t));
      lua_pop(L, 1);
    }
    printf("\n");
  }
  if (at >= len) return 0;
  lua_pushinteger(L, (lua_Integer)(at + step));
  lua_replace(L, lua_upvalueindex(3));
  lua_pushlstring(L, src + at, at + step > len ? len - at : step);
  return 1;
}

struct out { char *b; size_t n; };

static int dump_writer(lua_State *L, const void *p, size_t sz, void *ud) {
  struct out *o = (struct out *)ud;
  (void)L;
  o->b = (char *)realloc(o->b, o->n + sz);
  memcpy(o->b + o->n, p, sz);
  o->n += sz;
  return 0;
}

/* load(reader over `src` in pieces of `step`), reporting the outcome */
static void run(lua_State *L, const char *src, size_t len, int step) {
  lua_getglobal(L, "load");
  lua_pushlstring(L, src, len);
  lua_pushinteger(L, step);
  lua_pushinteger(L, 0);
  lua_pushcclosure(L, reader, 3);
  lua_pushstring(L, "=chunk");
  lua_call(L, 2, 2);
  printf("%s\n", lua_isfunction(L, -2) ? "loaded" : lua_tostring(L, -1));
  lua_pop(L, 2);
}

int main(void) {
  static const char text[] =
    "local x = 10 local s = 'str' local t = {1, 2.5, a = 'k'} "
    "local f = function(a, b) return a + b * 2 end return f(x, 1), t";
  struct out o = {NULL, 0};
  lua_State *L = luaL_newstate();
  luaL_openlibs(L);
  run(L, text, strlen(text), 7);
  luaL_loadstring(L, text);
#if LUA_VERSION_NUM >= 503
  lua_dump(L, dump_writer, &o, 0);
#else
  lua_dump(L, dump_writer, &o);
#endif
  lua_pop(L, 1);
  run(L, o.b, o.n, 9);
  free(o.b);
  lua_close(L);
  return 0;
}
