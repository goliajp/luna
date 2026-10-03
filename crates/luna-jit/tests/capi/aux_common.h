/* Shared helpers of the auxiliary library's test hosts. */
#ifndef AUX_COMMON_H
#define AUX_COMMON_H

#include <stdio.h>
#include <string.h>
#include "lua.h"
#include "lauxlib.h"
#include "lualib.h"

/* print len bytes, escaping what is not printable ASCII */
static void put_bytes(const char *s, size_t len) {
  size_t i;
  for (i = 0; i < len; i++) {
    unsigned char c = (unsigned char)s[i];
    if (c >= 0x20 && c < 0x7f && c != '\\')
      putchar(c);
    else
      printf("\\%d", c);
  }
}

/* one value, as a test reads it */
static void put_value(lua_State *L, int idx) {
  int t = lua_type(L, idx);
  switch (t) {
    case LUA_TSTRING: {
      size_t len;
      const char *s = lua_tolstring(L, idx, &len);
      putchar('[');
      put_bytes(s, len);
      putchar(']');
      break;
    }
    case LUA_TNUMBER:
      lua_pushvalue(L, idx);
      printf("%s", lua_tostring(L, -1));
      lua_pop(L, 1);
      break;
    case LUA_TBOOLEAN:
      printf("%s", lua_toboolean(L, idx) ? "true" : "false");
      break;
    case LUA_TNIL:
      printf("nil");
      break;
    default:
      printf("<%s>", lua_typename(L, t));
  }
}

/* label, top, and the values from `from` to the top */
static void show_from(lua_State *L, const char *label, int from) {
  int i, top = lua_gettop(L);
  printf("%s: top=%d", label, top);
  for (i = from; i <= top; i++) {
    putchar(' ');
    put_value(L, i);
  }
  putchar('\n');
}

/* call f with the values a chunk returns as its arguments, protected;
   print the status and the results or the error, then clear the stack */
static void run(lua_State *L, const char *label, lua_CFunction f, const char *args) {
  int st, base;
  lua_settop(L, 0);
  lua_pushcfunction(L, f);
  if (args != NULL) {
    if (luaL_loadstring(L, args) != 0 || lua_pcall(L, 0, LUA_MULTRET, 0) != 0) {
      printf("%s: setup failed: %s\n", label, lua_tostring(L, -1));
      lua_settop(L, 0);
      return;
    }
  }
  st = lua_pcall(L, lua_gettop(L) - 1, LUA_MULTRET, 0);
  base = 1;
  printf("%s: status=%d", label, st);
  show_from(L, "", base);
  lua_settop(L, 0);
}

/* run a chunk, print an error if it raised one */
static void dochunk(lua_State *L, const char *src) {
  if (luaL_dostring(L, src) != 0) {
    printf("error: ");
    put_value(L, -1);
    putchar('\n');
  }
  lua_settop(L, 0);
}

#endif
