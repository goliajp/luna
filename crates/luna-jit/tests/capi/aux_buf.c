/* luaL_Buffer: what each step leaves on the stack, growth past the
   initial buffer, luaL_addvalue below a growing buffer, and the result */
#include "aux_common.h"

#define BIG 100000

static char big[BIG];

/* 5.1's pieces on the stack depend on the C library's BUFSIZ, so only
   later versions show the stack while a buffer is in use */
#if LUA_VERSION_NUM >= 502
#define RELTOP(label) printf("%s: rel top=%d\n", label, lua_gettop(L) - base)
#else
#define RELTOP(label) ((void)0)
#endif

static void result(lua_State *L, const char *label, int base) {
  size_t len;
  const char *s = lua_tolstring(L, -1, &len);
  printf("%s: top=%d len=%d head=", label, lua_gettop(L) - base, (int)len);
  put_bytes(s, len < 12 ? len : 12);
  printf(" tail=");
  put_bytes(s + (len < 6 ? 0 : len - 6), len < 6 ? len : 6);
  printf("\n");
}

static int f_small(lua_State *L) {
  luaL_Buffer b;
  int base = lua_gettop(L);
  luaL_buffinit(L, &b);
  RELTOP("small init");
  luaL_addchar(&b, 'a');
  luaL_addstring(&b, "bc");
  luaL_addlstring(&b, "d\0e", 3);
  luaL_addlstring(&b, NULL, 0);
  RELTOP("small added");
  lua_pushinteger(L, 42);
  luaL_addvalue(&b);
  RELTOP("small addvalue");
  luaL_pushresult(&b);
  result(L, "small", base);
  return 0;
}

static int f_big(lua_State *L) {
  luaL_Buffer b;
  int base = lua_gettop(L);
  int i;
  luaL_buffinit(L, &b);
  luaL_addlstring(&b, big, BIG);
  RELTOP("big after 1");
  luaL_addlstring(&b, big, BIG);
  RELTOP("big after 2");
  for (i = 0; i < 1000; i++) luaL_addchar(&b, 'z');
  RELTOP("big chars");
  lua_pushlstring(L, big, BIG);
  luaL_addvalue(&b);
  RELTOP("big addvalue");
  luaL_pushresult(&b);
  result(L, "big", base);
  return 0;
}

/* a long value added while the buffer is still the initial one */
static int f_bigvalue(lua_State *L) {
  luaL_Buffer b;
  int base = lua_gettop(L);
  luaL_buffinit(L, &b);
  luaL_addstring(&b, "head:");
  lua_pushlstring(L, big, BIG);
  luaL_addvalue(&b);
  RELTOP("bigvalue");
  luaL_addstring(&b, ":tail");
  luaL_pushresult(&b);
  result(L, "bigvalue", base);
  return 0;
}

static int f_chars(lua_State *L) {
  luaL_Buffer b;
  int base = lua_gettop(L);
  int i;
  luaL_buffinit(L, &b);
  for (i = 0; i < BIG; i++) luaL_addchar(&b, (char)('0' + i % 10));
  luaL_pushresult(&b);
  result(L, "chars", base);
  return 0;
}

static int f_empty(lua_State *L) {
  luaL_Buffer b;
  int base = lua_gettop(L);
  luaL_buffinit(L, &b);
  luaL_pushresult(&b);
  result(L, "empty", base);
  return 0;
}

static int f_prep(lua_State *L) {
  luaL_Buffer b;
  int base = lua_gettop(L);
  char *p;
  luaL_buffinit(L, &b);
  p = luaL_prepbuffer(&b);
  memcpy(p, "prepared", 8);
  luaL_addsize(&b, 8);
#if LUA_VERSION_NUM >= 502
  p = luaL_prepbuffsize(&b, BIG);
  memset(p, 'q', BIG);
  luaL_addsize(&b, BIG);
  RELTOP("prep");
  luaL_pushresultsize(&b, 0);
#else
  luaL_pushresult(&b);
#endif
  result(L, "prep", base);
  return 0;
}

#if LUA_VERSION_NUM >= 502
static int f_initsize(lua_State *L) {
  luaL_Buffer b;
  int base = lua_gettop(L);
  char *p = luaL_buffinitsize(L, &b, 10);
  memcpy(p, "0123456789", 10);
  RELTOP("initsize small");
  luaL_pushresultsize(&b, 10);
  result(L, "initsize small", base);
  lua_settop(L, base);
  p = luaL_buffinitsize(L, &b, BIG);
  memset(p, 'w', BIG);
  RELTOP("initsize big");
  luaL_pushresultsize(&b, BIG);
  result(L, "initsize big", base);
  return 0;
}
#endif

#if LUA_VERSION_NUM >= 504
static int f_addgsub(lua_State *L) {
  luaL_Buffer b;
  int base = lua_gettop(L);
  luaL_buffinit(L, &b);
  luaL_addgsub(&b, "x-y-z", "-", "+");
  luaL_addgsub(&b, "!", "?", "");
  printf("addgsub: rel top=%d len=%d\n", lua_gettop(L) - base, (int)luaL_bufflen(&b));
  luaL_buffsub(&b, 1);
  luaL_pushresult(&b);
  result(L, "addgsub", base);
  return 0;
}
#endif

/* an error while the buffer has grown: the box goes with the error */
static int f_error(lua_State *L) {
  luaL_Buffer b;
  luaL_buffinit(L, &b);
  luaL_addlstring(&b, big, BIG);
  return luaL_error(L, "error with buffer");
}

int main(void) {
  lua_State *L = luaL_newstate();
  int i;
  for (i = 0; i < BIG; i++) big[i] = (char)('a' + i % 26);
  luaL_openlibs(L);
  lua_pushstring(L, "under");
  run(L, "small", f_small, NULL);
  run(L, "big", f_big, NULL);
  run(L, "bigvalue", f_bigvalue, NULL);
  run(L, "chars", f_chars, NULL);
  run(L, "empty", f_empty, NULL);
  run(L, "prep", f_prep, NULL);
#if LUA_VERSION_NUM >= 502
  run(L, "initsize", f_initsize, NULL);
#endif
#if LUA_VERSION_NUM >= 504
  run(L, "addgsub", f_addgsub, NULL);
#endif
  run(L, "error", f_error, NULL);
  /* called straight from the host, below other values */
  lua_settop(L, 0);
  lua_pushstring(L, "x");
  f_big(L);
  printf("host big: top=%d\n", lua_gettop(L));
  lua_settop(L, 0);
  dochunk(L, "collectgarbage() collectgarbage() print('collected')");
  printf("top at end=%d\n", lua_gettop(L));
  lua_close(L);
  return 0;
}
