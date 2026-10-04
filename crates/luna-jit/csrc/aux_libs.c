/*
 * The standard libraries one at a time (luaopen_*), opening all of them
 * (luaL_openlibs, 5.5's luaL_openselectedlibs), and 5.1/5.2's module
 * functions (luaL_register, luaL_openlib, luaL_pushmodule,
 * luaL_findtable).
 */
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include "auxlib.h"

/* the Rust side: open library `name` into L's stack; returns how many
   values it pushed, or -1 after pushing the name of the module whose
   global is in the way (5.1) */
int luna_capi_openlib(lua_State *L, const char *name);

static int openlib(lua_State *L, const char *name) {
  int n = luna_capi_openlib(L, name);
  if (n < 0) {
    const char *mod = lua_tostring(L, -1);
    luna_c_luaL_error(L, "name conflict for module '%s'", mod);
  }
  return n;
}

#define OPENER(fn, name) \
  LUNA_HIDDEN int luna_c_##fn(lua_State *L) { return openlib(L, name); }

OPENER(luaopen_base, "_G")
OPENER(luaopen_coroutine, "coroutine")
OPENER(luaopen_table, "table")
OPENER(luaopen_os, "os")
OPENER(luaopen_string, "string")
OPENER(luaopen_bit32, "bit32")
OPENER(luaopen_math, "math")
OPENER(luaopen_utf8, "utf8")
OPENER(luaopen_debug, "debug")

int luna_io_open51(lua_State *L);
int luna_io_open52(lua_State *L);

/* io is C, over the C library's stdio (io_*.c) */
LUNA_HIDDEN int luna_c_luaopen_io(lua_State *L) {
  return VNUM(L) == 501 ? luna_io_open51(L) : luna_io_open52(L);
}

/* the finalizer of the table of loaded C libraries; luna loads none */
static int gctm(lua_State *L) {
  (void)L;
  return 0;
}

static const int clibs53 = 0;

/* the table of loaded C libraries luaopen_package makes in the registry:
   5.1's "_LOADLIB" metatable, "_CLIBS" (5.3: a light userdata key) with a
   finalizer; 5.1, 5.2 and 5.4 leave it on the stack */
LUNA_HIDDEN int luna_c_luaopen_package(lua_State *L) {
  switch (VNUM(L)) {
    case 501:
      luna_c_luaL_newmetatable(L, "_LOADLIB");
      lua_pushcfunction(L, gctm);
      lua_setfield(L, -2, "__gc");
      break;
    case 503:
      lua_newtable(L);
      lua_createtable(L, 0, 1);
      lua_pushcfunction(L, gctm);
      lua_setfield(L, -2, "__gc");
      lua_setmetatable(L, -2);
      lua_rawsetp(L, REGIDX(L), &clibs53);
      break;
    case 505:
      luna_c_luaL_getsubtable(L, REGIDX(L), "_CLIBS");
      lua_pop(L, 1);
      break;
    default:
      luna_c_luaL_getsubtable(L, REGIDX(L), "_CLIBS");
      lua_createtable(L, 0, 1);
      lua_pushcfunction(L, gctm);
      lua_setfield(L, -2, "__gc");
      lua_setmetatable(L, -2);
  }
  return openlib(L, "package");
}

/* each version's luaL_openlibs list, in its order */
static const luaL_Reg libs51[] = {
    {"", luna_c_luaopen_base},          {"package", luna_c_luaopen_package},
    {"table", luna_c_luaopen_table},    {"io", luna_c_luaopen_io},
    {"os", luna_c_luaopen_os},          {"string", luna_c_luaopen_string},
    {"math", luna_c_luaopen_math},      {"debug", luna_c_luaopen_debug},
    {NULL, NULL}};

static const luaL_Reg libs52[] = {
    {"_G", luna_c_luaopen_base},         {"package", luna_c_luaopen_package},
    {"coroutine", luna_c_luaopen_coroutine}, {"table", luna_c_luaopen_table},
    {"io", luna_c_luaopen_io},           {"os", luna_c_luaopen_os},
    {"string", luna_c_luaopen_string},   {"bit32", luna_c_luaopen_bit32},
    {"math", luna_c_luaopen_math},       {"debug", luna_c_luaopen_debug},
    {NULL, NULL}};

static const luaL_Reg libs53[] = {
    {"_G", luna_c_luaopen_base},         {"package", luna_c_luaopen_package},
    {"coroutine", luna_c_luaopen_coroutine}, {"table", luna_c_luaopen_table},
    {"io", luna_c_luaopen_io},           {"os", luna_c_luaopen_os},
    {"string", luna_c_luaopen_string},   {"math", luna_c_luaopen_math},
    {"utf8", luna_c_luaopen_utf8},       {"debug", luna_c_luaopen_debug},
    {"bit32", luna_c_luaopen_bit32},     {NULL, NULL}};

/* 5.4's list is 5.3's without bit32 */
#define libs54 libs53

static const luaL_Reg libs55[] = {
    {"_G", luna_c_luaopen_base},         {"package", luna_c_luaopen_package},
    {"coroutine", luna_c_luaopen_coroutine}, {"debug", luna_c_luaopen_debug},
    {"io", luna_c_luaopen_io},           {"math", luna_c_luaopen_math},
    {"os", luna_c_luaopen_os},           {"string", luna_c_luaopen_string},
    {"table", luna_c_luaopen_table},     {"utf8", luna_c_luaopen_utf8},
    {NULL, NULL}};

LUNA_HIDDEN void luna_c_luaL_openselectedlibs(lua_State *L, int load, int preload) {
  int mask;
  const luaL_Reg *lib;
  luna_c_luaL_getsubtable(L, REGIDX(L), "_PRELOAD");
  for (lib = libs55, mask = 1; lib->name != NULL; lib++, mask <<= 1) {
    if (load & mask) {
      luna_c_luaL_requiref(L, lib->name, lib->func, 1);
      lua_pop(L, 1);
    }
    else if (preload & mask) {
      lua_pushcfunction(L, lib->func);
      lua_setfield(L, -2, lib->name);
    }
  }
  lua_pop(L, 1);
}

LUNA_HIDDEN void luna_c_luaL_openlibs(lua_State *L) {
  const luaL_Reg *lib;
  switch (VNUM(L)) {
    case 501:
      for (lib = libs51; lib->func; lib++) {
        lua_pushcfunction(L, lib->func);
        lua_pushstring(L, lib->name);
        lua_call(L, 1, 0);
      }
      return;
    case 505:
      luna_c_luaL_openselectedlibs(L, ~0, 0);
      return;
    default:
      lib = VNUM(L) == 502 ? libs52 : libs53;
      for (; lib->func; lib++) {
        if (VNUM(L) >= 504 && lib->func == luna_c_luaopen_bit32) continue;
        luna_c_luaL_requiref(L, lib->name, lib->func, 1);
        lua_pop(L, 1);
      }
      if (VNUM(L) == 502) {
        luna_c_luaL_getsubtable(L, REGIDX(L), "_PRELOAD");
        lua_pop(L, 1);
      }
  }
}

/* PUC 5.5 luai_makeseed: an address and the time */
LUNA_HIDDEN unsigned int luna_c_luaL_makeseed(lua_State *L) {
  unsigned int buff[(sizeof(void *) + sizeof(time_t) + sizeof(int) - 1) / sizeof(int)];
  unsigned int res, i;
  time_t t = time(NULL);
  char *b = (char *)buff;
  (void)L;
  memset(buff, 0, sizeof(buff));
  memcpy(b, (void *)&b, sizeof(char *));
  memcpy(b + sizeof(char *), &t, sizeof(t));
  res = buff[0];
  for (i = 1; i < sizeof(buff) / sizeof(buff[0]); i++) res ^= (res >> 3) + (res << 7) + buff[i];
  return res;
}

LUNA_HIDDEN void *luna_c_luaL_alloc(void *ud, void *ptr, size_t osize, size_t nsize) {
  (void)ud;
  (void)osize;
  if (nsize == 0) {
    free(ptr);
    return NULL;
  }
  return realloc(ptr, nsize);
}

/* 5.1's public luaL_findtable; 5.2's private one takes idx 0 for the
   table on top */
LUNA_HIDDEN const char *luna_c_luaL_findtable(lua_State *L, int idx, const char *fname,
                                              int szhint) {
  const char *e;
  if (idx || VNUM(L) == 501) lua_pushvalue(L, idx);
  do {
    e = strchr(fname, '.');
    if (e == NULL) e = fname + strlen(fname);
    lua_pushlstring(L, fname, (size_t)(e - fname));
    if (lua_rawget(L, -2) == LUA_TNIL) {
      lua_pop(L, 1);
      lua_createtable(L, 0, (*e == '.' ? 1 : szhint));
      lua_pushlstring(L, fname, (size_t)(e - fname));
      lua_pushvalue(L, -2);
      lua_settable(L, -4);
    }
    else if (!lua_istable(L, -1)) {
      lua_pop(L, 2);
      return fname;
    }
    lua_remove(L, -2);
    fname = e + 1;
  } while (*e == '.');
  return NULL;
}

static int libsize(const luaL_Reg *l) {
  int size = 0;
  for (; l && l->name; l++) size++;
  return size;
}

LUNA_HIDDEN void luna_c_luaL_pushmodule(lua_State *L, const char *modname, int sizehint) {
  luna_c_luaL_findtable(L, REGIDX(L), "_LOADED", 1);
  if (lua_getfield(L, -1, modname) != LUA_TTABLE) {
    const char *clash;
    lua_pop(L, 1);
    if (VNUM(L) == 501)
      clash = luna_c_luaL_findtable(L, GLOBALSINDEX_51, modname, sizehint);
    else {
      luna_aux_pushglobaltable(L);
      clash = luna_c_luaL_findtable(L, 0, modname, sizehint);
    }
    if (clash != NULL)
      luna_c_luaL_error(L, "name conflict for module '%s'", modname);
    lua_pushvalue(L, -1);
    lua_setfield(L, -3, modname);
  }
  lua_remove(L, -2);
}

/* 5.1 opens into the module table without luaL_setfuncs's stack check;
   5.2 goes through luaL_pushmodule and luaL_setfuncs */
LUNA_HIDDEN void luna_c_luaL_openlib(lua_State *L, const char *libname, const luaL_Reg *l,
                                     int nup) {
  if (VNUM(L) == 501) {
    if (libname) {
      luna_c_luaL_pushmodule(L, libname, libsize(l));
      lua_insert(L, -(nup + 1));
    }
    for (; l->name; l++) {
      int i;
      for (i = 0; i < nup; i++) lua_pushvalue(L, -nup);
      lua_pushcclosure(L, l->func, nup);
      lua_setfield(L, -(nup + 2), l->name);
    }
    lua_pop(L, nup);
    return;
  }
  if (libname) {
    luna_c_luaL_pushmodule(L, libname, libsize(l));
    lua_insert(L, -(nup + 1));
  }
  if (l)
    luna_c_luaL_setfuncs(L, l, nup);
  else
    lua_pop(L, nup);
}

LUNA_HIDDEN void luna_c_luaL_register(lua_State *L, const char *libname, const luaL_Reg *l) {
  luna_c_luaL_openlib(L, libname, l, 0);
}
