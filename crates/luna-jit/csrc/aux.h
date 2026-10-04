/*
 * The auxiliary library (lauxlib) and lua_pushfstring, written on top of
 * the public API as PUC writes them. One build serves every dialect: each
 * function reads the state's version (G(L)->version) where PUC's versions
 * differ. The API functions are declared here with their 5.4/5.5 shapes,
 * which the exported symbols have.
 */
#ifndef LUNA_AUX_H
#define LUNA_AUX_H

#include <limits.h>
#include <stdio.h>
#include "shim.h"

#define VNUM(L) (G(L)->version)

#define LUA_MULTRET (-1)
#define LUA_ERRERR_OF(L) ((VNUM(L) == 502 || VNUM(L) == 503) ? 6 : 5)

#define LUA_TNONE (-1)
#define LUA_TNIL 0
#define LUA_TBOOLEAN 1
#define LUA_TLIGHTUSERDATA 2
#define LUA_TNUMBER 3
#define LUA_TSTRING 4
#define LUA_TTABLE 5
#define LUA_TFUNCTION 6
#define LUA_TUSERDATA 7
#define LUA_TTHREAD 8

#define LUA_REFNIL (-1)
#define LUA_IDSIZE 60
#define LUA_MINSTACK 20
#define LUA_GCSTEP 5

/* LUA_REGISTRYINDEX of the state's dialect, and 5.1's LUA_GLOBALSINDEX */
#define REGIDX(L)                                                      \
  (VNUM(L) == 501 ? -10000                                             \
                  : VNUM(L) == 505 ? -(INT_MAX / 2 + 1000) : -1001000)
#define GLOBALSINDEX_51 (-10002)
#define LUA_RIDX_GLOBALS 2

typedef const char *(*lua_Reader)(lua_State *L, void *ud, size_t *sz);
typedef void *(*lua_Alloc)(void *ud, void *ptr, size_t osize, size_t nsize);
typedef void (*lua_WarnFunction)(void *ud, const char *msg, int tocont);

typedef struct luaL_Reg {
  const char *name;
  lua_CFunction func;
} luaL_Reg;

/* the public API, as exported */
int lua_gettop(lua_State *L);
void lua_settop(lua_State *L, int idx);
int lua_absindex(lua_State *L, int idx);
void lua_pushvalue(lua_State *L, int idx);
void lua_rotate(lua_State *L, int idx, int n);
void lua_copy(lua_State *L, int from, int to);
void lua_insert(lua_State *L, int idx);
void lua_remove(lua_State *L, int idx);
void lua_replace(lua_State *L, int idx);
int lua_checkstack(lua_State *L, int n);
int lua_type(lua_State *L, int idx);
const char *lua_typename(lua_State *L, int t);
int lua_isnumber(lua_State *L, int idx);
int lua_isstring(lua_State *L, int idx);
int lua_isinteger(lua_State *L, int idx);
lua_Number lua_tonumberx(lua_State *L, int idx, int *isnum);
lua_Integer lua_tointegerx(lua_State *L, int idx, int *isnum);
int lua_toboolean(lua_State *L, int idx);
const char *lua_tolstring(lua_State *L, int idx, size_t *len);
size_t lua_rawlen(lua_State *L, int idx);
size_t lua_objlen(lua_State *L, int idx);
void *lua_touserdata(lua_State *L, int idx);
const void *lua_topointer(lua_State *L, int idx);
int lua_rawequal(lua_State *L, int a, int b);
unsigned lua_numbertocstring(lua_State *L, int idx, char *buff);
void lua_pushnil(lua_State *L);
void lua_pushnumber(lua_State *L, lua_Number n);
void lua_pushinteger(lua_State *L, lua_Integer n);
const char *lua_pushlstring(lua_State *L, const char *s, size_t len);
const char *lua_pushstring(lua_State *L, const char *s);
const char *lua_pushexternalstring(lua_State *L, const char *s, size_t len,
                                   lua_Alloc falloc, void *ud);
void lua_pushcclosure(lua_State *L, lua_CFunction fn, int n);
void lua_pushboolean(lua_State *L, int b);
void lua_pushlightuserdata(lua_State *L, void *p);
int lua_getfield(lua_State *L, int idx, const char *k);
void lua_setfield(lua_State *L, int idx, const char *k);
int lua_rawget(lua_State *L, int idx);
int lua_rawgeti(lua_State *L, int idx, lua_Integer n);
void lua_rawset(lua_State *L, int idx);
void lua_rawseti(lua_State *L, int idx, lua_Integer n);
void lua_rawsetp(lua_State *L, int idx, const void *p);
void lua_settable(lua_State *L, int idx);
void lua_createtable(lua_State *L, int narr, int nrec);
void *lua_newuserdatauv(lua_State *L, size_t sz, int nuvalue);
int lua_getmetatable(lua_State *L, int idx);
int lua_setmetatable(lua_State *L, int idx);
int lua_next(lua_State *L, int idx);
void lua_len(lua_State *L, int idx);
void lua_concat(lua_State *L, int n);
int lua_error(lua_State *L);
void lua_setglobal(lua_State *L, const char *name);
void lua_call(lua_State *L, int nargs, int nresults);
int lua_load(lua_State *L, lua_Reader reader, void *dt, const char *chunkname,
             const char *mode);
int luna_load_51(lua_State *L, lua_Reader reader, void *dt, const char *chunkname);
lua_Alloc lua_getallocf(lua_State *L, void **ud);
int lua_gc(lua_State *L, int what, ...);
int lua_getstack(lua_State *L, int level, lua_Debug *ar);
int lua_getinfo(lua_State *L, const char *what, lua_Debug *ar);
void lua_toclose(lua_State *L, int idx);
void lua_closeslot(lua_State *L, int idx);
lua_Number lua_version(lua_State *L);

#define lua_pop(L, n) lua_settop(L, -(n)-1)
#define lua_newtable(L) lua_createtable(L, 0, 0)
#define lua_pushcfunction(L, f) lua_pushcclosure(L, (f), 0)
#define lua_pushliteral(L, s) lua_pushstring(L, "" s)
#define lua_isnil(L, n) (lua_type(L, (n)) == LUA_TNIL)
#define lua_istable(L, n) (lua_type(L, (n)) == LUA_TTABLE)
#define lua_isnoneornil(L, n) (lua_type(L, (n)) <= 0)
#define lua_tostring(L, i) lua_tolstring(L, (i), NULL)
#define lua_tointeger(L, i) lua_tointegerx(L, (i), NULL)
#define lua_tonumber(L, i) lua_tonumberx(L, (i), NULL)
#define luaL_typename(L, i) lua_typename(L, lua_type(L, (i)))

/* functions of this family other files of it call */
const char *luna_c_lua_pushfstring(lua_State *L, const char *fmt, ...);
const char *luna_c_lua_pushvfstring(lua_State *L, const char *fmt, va_list argp);
int luna_c_luaL_error(lua_State *L, const char *fmt, ...);
void luna_c_luaL_where(lua_State *L, int level);
int luna_c_luaL_argerror(lua_State *L, int arg, const char *extramsg);
int luna_c_luaL_typeerror(lua_State *L, int arg, const char *tname);
void luna_c_luaL_checkstack(lua_State *L, int space, const char *msg);
int luna_c_luaL_getmetafield(lua_State *L, int obj, const char *event);
int luna_c_luaL_callmeta(lua_State *L, int obj, const char *event);
const char *luna_c_luaL_checklstring(lua_State *L, int arg, size_t *len);
const char *luna_c_luaL_optlstring(lua_State *L, int arg, const char *def,
                                   size_t *len);
int luna_c_luaL_getsubtable(lua_State *L, int idx, const char *fname);
void luna_c_luaL_setfuncs(lua_State *L, const luaL_Reg *l, int nup);
void luna_c_luaL_requiref(lua_State *L, const char *modname, lua_CFunction openf,
                          int glb);
int luna_c_luaL_newmetatable(lua_State *L, const char *tname);
void luna_c_luaL_checkversion_(lua_State *L, lua_Number ver, size_t sz);
void *luna_c_luaL_alloc(void *ud, void *ptr, size_t osize, size_t nsize);

/* the parts of lua_Debug lauxlib reads, whatever the dialect's layout */
struct aux_ar {
  const char *name;
  const char *namewhat;
  const char *what;
  const char *short_src;
  int currentline;
  int linedefined;
  int istailcall;
  int extraargs;
};

/* room for any dialect's lua_Debug */
typedef union aux_debug {
  char bytes[256];
  void *align;
} aux_debug;

/* lua_getinfo on `d`, read into `out` */
void luna_aux_getinfo(lua_State *L, const char *what, aux_debug *d,
                      struct aux_ar *out);
/* the name of the function `d` describes among the loaded modules (5.2:
   the globals): pushes it and returns 1, or returns 0 */
int luna_aux_pushglobalfuncname(lua_State *L, aux_debug *d);

/* lua_pushglobaltable of the dialect */
void luna_aux_pushglobaltable(lua_State *L);

#endif
