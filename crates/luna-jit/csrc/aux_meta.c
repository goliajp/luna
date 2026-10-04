/*
 * lauxlib's metatable and conversion helpers, references, and the results
 * of file and process operations.
 */
#include <errno.h>
#include <string.h>
#include "auxlib.h"
#if !defined(_WIN32)
#include <sys/wait.h>
#endif

LUNA_HIDDEN int luna_c_luaL_newmetatable(lua_State *L, const char *tname) {
  if (lua_getfield(L, REGIDX(L), tname) != LUA_TNIL) return 0;
  lua_pop(L, 1);
  if (VNUM(L) >= 503) {
    lua_createtable(L, 0, 2);
    lua_pushstring(L, tname);
    lua_setfield(L, -2, "__name");
  }
  else
    lua_newtable(L);
  lua_pushvalue(L, -1);
  lua_setfield(L, REGIDX(L), tname);
  return 1;
}

LUNA_HIDDEN void luna_c_luaL_setmetatable(lua_State *L, const char *tname) {
  lua_getfield(L, REGIDX(L), tname);
  lua_setmetatable(L, -2);
}

LUNA_HIDDEN void *luna_c_luaL_testudata(lua_State *L, int ud, const char *tname) {
  void *p = lua_touserdata(L, ud);
  if (p != NULL && lua_getmetatable(L, ud)) {
    lua_getfield(L, REGIDX(L), tname);
    if (!lua_rawequal(L, -1, -2)) p = NULL;
    lua_pop(L, 2);
    return p;
  }
  return NULL;
}

/* 5.1 leaves both metatables on the stack when the check fails; the
   error removes them anyway */
LUNA_HIDDEN void *luna_c_luaL_checkudata(lua_State *L, int ud, const char *tname) {
  void *p = luna_c_luaL_testudata(L, ud, tname);
  if (p == NULL) luna_c_luaL_typeerror(L, ud, tname);
  return p;
}

/* the metafield's type from 5.3 on; earlier, whether there is one */
LUNA_HIDDEN int luna_c_luaL_getmetafield(lua_State *L, int obj, const char *event) {
  int tt;
  if (!lua_getmetatable(L, obj)) return LUA_TNIL;
  lua_pushstring(L, event);
  tt = lua_rawget(L, -2);
  if (tt == LUA_TNIL) {
    lua_pop(L, 2);
    return LUA_TNIL;
  }
  lua_remove(L, -2);
  return VNUM(L) >= 503 ? tt : 1;
}

LUNA_HIDDEN int luna_c_luaL_callmeta(lua_State *L, int obj, const char *event) {
  obj = lua_absindex(L, obj);
  if (luna_c_luaL_getmetafield(L, obj, event) == LUA_TNIL) return 0;
  lua_pushvalue(L, obj);
  lua_call(L, 1, 1);
  return 1;
}

LUNA_HIDDEN lua_Integer luna_c_luaL_len(lua_State *L, int idx) {
  int isnum;
  lua_Integer l;
  lua_len(L, idx);
  l = lua_tointegerx(L, -1, &isnum);
  if (!isnum)
    luna_c_luaL_error(L, VNUM(L) == 502 ? "object length is not a number"
                                        : "object length is not an integer");
  lua_pop(L, 1);
  return l;
}

LUNA_HIDDEN const char *luna_c_luaL_tolstring(lua_State *L, int idx, size_t *len) {
  int v = VNUM(L);
  if (v >= 504) idx = lua_absindex(L, idx);
  if (luna_c_luaL_callmeta(L, idx, "__tostring")) {
    if (v >= 503 && !lua_isstring(L, -1))
      luna_c_luaL_error(L, "'__tostring' must return a string");
  }
  else {
    switch (lua_type(L, idx)) {
      case LUA_TNUMBER:
        if (v == 502)
          lua_pushvalue(L, idx);
        else if (v >= 505) {
          char buff[64];
          lua_numbertocstring(L, idx, buff);
          lua_pushstring(L, buff);
        }
        else if (lua_isinteger(L, idx))
          luna_c_lua_pushfstring(L, "%I", (long long)lua_tointeger(L, idx));
        else
          luna_c_lua_pushfstring(L, "%f", (double)lua_tonumber(L, idx));
        break;
      case LUA_TSTRING:
        lua_pushvalue(L, idx);
        break;
      case LUA_TBOOLEAN:
        lua_pushstring(L, lua_toboolean(L, idx) ? "true" : "false");
        break;
      case LUA_TNIL:
        lua_pushliteral(L, "nil");
        break;
      default: {
        int tt = v >= 503 ? luna_c_luaL_getmetafield(L, idx, "__name") : LUA_TNIL;
        const char *kind = tt == LUA_TSTRING ? lua_tostring(L, -1) : luaL_typename(L, idx);
        luna_c_lua_pushfstring(L, "%s: %p", kind, lua_topointer(L, idx));
        if (tt != LUA_TNIL) lua_remove(L, -2);
        break;
      }
    }
  }
  return lua_tolstring(L, -1, len);
}

LUNA_HIDDEN int luna_c_luaL_getsubtable(lua_State *L, int idx, const char *fname) {
  if (lua_getfield(L, idx, fname) == LUA_TTABLE) return 1;
  lua_pop(L, 1);
  idx = lua_absindex(L, idx);
  lua_newtable(L);
  lua_pushvalue(L, -1);
  lua_setfield(L, idx, fname);
  return 0;
}

/* 5.2 always calls openf; later versions only when package.loaded has no
   true value for modname */
LUNA_HIDDEN void luna_c_luaL_requiref(lua_State *L, const char *modname,
                                      lua_CFunction openf, int glb) {
  if (VNUM(L) == 502) {
    lua_pushcfunction(L, openf);
    lua_pushstring(L, modname);
    lua_call(L, 1, 1);
    luna_c_luaL_getsubtable(L, REGIDX(L), "_LOADED");
    lua_pushvalue(L, -2);
    lua_setfield(L, -2, modname);
    lua_pop(L, 1);
  }
  else {
    luna_c_luaL_getsubtable(L, REGIDX(L), "_LOADED");
    lua_getfield(L, -1, modname);
    if (!lua_toboolean(L, -1)) {
      lua_pop(L, 1);
      lua_pushcfunction(L, openf);
      lua_pushstring(L, modname);
      lua_call(L, 1, 1);
      lua_pushvalue(L, -1);
      lua_setfield(L, -3, modname);
    }
    lua_remove(L, -2);
  }
  if (glb) {
    lua_pushvalue(L, -1);
    lua_setglobal(L, modname);
  }
}

/* 5.4 on store false for an entry without a function */
LUNA_HIDDEN void luna_c_luaL_setfuncs(lua_State *L, const luaL_Reg *l, int nup) {
  luna_c_luaL_checkstack(L, nup, "too many upvalues");
  for (; l->name != NULL; l++) {
    if (VNUM(L) >= 504 && l->func == NULL)
      lua_pushboolean(L, 0);
    else {
      int i;
      for (i = 0; i < nup; i++) lua_pushvalue(L, -nup);
      lua_pushcclosure(L, l->func, nup);
    }
    lua_setfield(L, -(nup + 2), l->name);
  }
  lua_pop(L, nup);
}

/* the free list: t[0] up to 5.3, t[3] in 5.4, t[1] in 5.5 */
LUNA_HIDDEN int luna_c_luaL_ref(lua_State *L, int t) {
  int v = VNUM(L), ref;
  lua_Integer freelist = v <= 503 ? 0 : v == 504 ? 3 : 1;
  if (lua_isnil(L, -1)) {
    lua_pop(L, 1);
    return LUA_REFNIL;
  }
  t = lua_absindex(L, t);
  if (v <= 503) {
    lua_rawgeti(L, t, freelist);
    ref = (int)lua_tointeger(L, -1);
  }
  else {
    int tt = lua_rawgeti(L, t, freelist);
    if (v == 504 ? tt != LUA_TNIL : tt == LUA_TNUMBER)
      ref = (int)lua_tointeger(L, -1);
    else {
      ref = 0;
      lua_pushinteger(L, 0);
      lua_rawseti(L, t, freelist);
    }
  }
  lua_pop(L, 1);
  if (ref != 0) {
    lua_rawgeti(L, t, ref);
    lua_rawseti(L, t, freelist);
  }
  else
    ref = (int)(v == 501 ? lua_objlen(L, t) : lua_rawlen(L, t)) + 1;
  lua_rawseti(L, t, ref);
  return ref;
}

LUNA_HIDDEN void luna_c_luaL_unref(lua_State *L, int t, int ref) {
  int v = VNUM(L);
  lua_Integer freelist = v <= 503 ? 0 : v == 504 ? 3 : 1;
  if (ref >= 0) {
    t = lua_absindex(L, t);
    lua_rawgeti(L, t, freelist);
    lua_rawseti(L, t, ref);
    lua_pushinteger(L, ref);
    lua_rawseti(L, t, freelist);
  }
}

LUNA_HIDDEN int luna_c_luaL_fileresult(lua_State *L, int stat, const char *fname) {
  int en = errno;
  if (stat) {
    lua_pushboolean(L, 1);
    return 1;
  }
  else {
    const char *msg = (VNUM(L) <= 503 || en != 0) ? strerror(en) : "(no extra info)";
    lua_pushnil(L);
    if (fname)
      luna_c_lua_pushfstring(L, "%s: %s", fname, msg);
    else
      lua_pushstring(L, msg);
    lua_pushinteger(L, en);
    return 3;
  }
}

LUNA_HIDDEN int luna_c_luaL_execresult(lua_State *L, int stat) {
  const char *what = "exit";
  if (VNUM(L) <= 503 ? stat == -1 : (stat != 0 && errno != 0))
    return luna_c_luaL_fileresult(L, 0, NULL);
#if !defined(_WIN32)
  if (WIFEXITED(stat))
    stat = WEXITSTATUS(stat);
  else if (WIFSIGNALED(stat)) {
    stat = WTERMSIG(stat);
    what = "signal";
  }
#endif
  if (*what == 'e' && stat == 0)
    lua_pushboolean(L, 1);
  else
    lua_pushnil(L);
  lua_pushstring(L, what);
  lua_pushinteger(L, stat);
  return 3;
}

/* luna has one core, so 5.2 and 5.3's "multiple Lua VMs" cannot happen */
LUNA_HIDDEN void luna_c_luaL_checkversion_(lua_State *L, lua_Number ver, size_t sz) {
  lua_Number v = (lua_Number)VNUM(L);
  if (VNUM(L) >= 503 && sz != 136)
    luna_c_luaL_error(L, "core and library have incompatible numeric types");
  else if (v != ver)
    luna_c_luaL_error(L, "version mismatch: app. needs %f, Lua core provides %f", ver, v);
}

LUNA_HIDDEN void luna_c_luna_checkversion_52(lua_State *L, lua_Number ver) {
  luna_c_luaL_checkversion_(L, ver, 136);
}
