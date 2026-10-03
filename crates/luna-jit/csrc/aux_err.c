/*
 * lauxlib's error reports: luaL_where, luaL_error, luaL_argerror and the
 * type errors, and reading lua_Debug in each dialect's layout.
 */
#include <string.h>
#include "aux.h"

/* lua_Debug of 5.1, 5.2/5.3, 5.4 and 5.5 */
struct dbg51 {
  int event;
  const char *name, *namewhat, *what, *source;
  int currentline, nups, linedefined, lastlinedefined;
  char short_src[LUA_IDSIZE];
  int i_ci;
};

struct dbg52 {
  int event;
  const char *name, *namewhat, *what, *source;
  int currentline, linedefined, lastlinedefined;
  unsigned char nups, nparams;
  char isvararg, istailcall;
  char short_src[LUA_IDSIZE];
  void *i_ci;
};

struct dbg54 {
  int event;
  const char *name, *namewhat, *what, *source;
  size_t srclen;
  int currentline, linedefined, lastlinedefined;
  unsigned char nups, nparams;
  char isvararg, istailcall;
  unsigned short ftransfer, ntransfer;
  char short_src[LUA_IDSIZE];
  void *i_ci;
};

struct dbg55 {
  int event;
  const char *name, *namewhat, *what, *source;
  size_t srclen;
  int currentline, linedefined, lastlinedefined;
  unsigned char nups, nparams;
  char isvararg;
  unsigned char extraargs;
  char istailcall;
  int ftransfer, ntransfer;
  char short_src[LUA_IDSIZE];
  void *i_ci;
};

#define COPY(p)                          \
  do {                                   \
    out->name = (p)->name;               \
    out->namewhat = (p)->namewhat;       \
    out->what = (p)->what;               \
    out->short_src = (p)->short_src;     \
    out->currentline = (p)->currentline; \
    out->linedefined = (p)->linedefined; \
  } while (0)

LUNA_HIDDEN void luna_aux_getinfo(lua_State *L, const char *what, aux_debug *d,
                                  struct aux_ar *out) {
  lua_getinfo(L, what, (lua_Debug *)d);
  out->istailcall = 0;
  out->extraargs = 0;
  switch (VNUM(L)) {
    case 501: {
      struct dbg51 *p = (struct dbg51 *)d;
      COPY(p);
      break;
    }
    case 502:
    case 503: {
      struct dbg52 *p = (struct dbg52 *)d;
      COPY(p);
      out->istailcall = p->istailcall;
      break;
    }
    case 504: {
      struct dbg54 *p = (struct dbg54 *)d;
      COPY(p);
      out->istailcall = p->istailcall;
      break;
    }
    default: {
      struct dbg55 *p = (struct dbg55 *)d;
      COPY(p);
      out->istailcall = p->istailcall;
      out->extraargs = p->extraargs;
      break;
    }
  }
}

LUNA_HIDDEN void luna_aux_pushglobaltable(lua_State *L) {
  if (VNUM(L) == 501)
    lua_pushvalue(L, GLOBALSINDEX_51);
  else
    lua_rawgeti(L, REGIDX(L), LUA_RIDX_GLOBALS);
}

LUNA_HIDDEN void luna_c_luaL_where(lua_State *L, int level) {
  aux_debug d;
  struct aux_ar ar;
  if (lua_getstack(L, level, (lua_Debug *)&d)) {
    luna_aux_getinfo(L, "Sl", &d, &ar);
    if (ar.currentline > 0) {
      luna_c_lua_pushfstring(L, "%s:%d: ", ar.short_src, ar.currentline);
      return;
    }
  }
  if (VNUM(L) <= 502)
    lua_pushliteral(L, "");
  else
    luna_c_lua_pushfstring(L, "");
}

LUNA_HIDDEN int luna_c_luaL_error(lua_State *L, const char *fmt, ...) {
  va_list argp;
  va_start(argp, fmt);
  luna_c_luaL_where(L, 1);
  luna_c_lua_pushvfstring(L, fmt, argp);
  va_end(argp);
  lua_concat(L, 2);
  return lua_error(L);
}

LUNA_HIDDEN int luna_c_luaL_argerror(lua_State *L, int arg, const char *extramsg) {
  aux_debug d;
  struct aux_ar ar;
  int v = VNUM(L);
  const char *argword = "argument";
  if (!lua_getstack(L, 0, (lua_Debug *)&d))
    return luna_c_luaL_error(L, "bad argument #%d (%s)", arg, extramsg);
  luna_aux_getinfo(L, v >= 505 ? "nt" : "n", &d, &ar);
  if (arg <= ar.extraargs)
    argword = "extra argument";
  else {
    arg -= ar.extraargs;
    if (strcmp(ar.namewhat, "method") == 0) {
      arg--;
      if (arg == 0)
        return luna_c_luaL_error(L, "calling '%s' on bad self (%s)", ar.name, extramsg);
    }
  }
  if (ar.name == NULL) {
    if (v == 501)
      ar.name = "?";
    else
      ar.name = luna_aux_pushglobalfuncname(L, &d) ? lua_tostring(L, -1) : "?";
  }
  if (v >= 505)
    return luna_c_luaL_error(L, "bad %s #%d to '%s' (%s)", argword, arg, ar.name, extramsg);
  return luna_c_luaL_error(L, "bad argument #%d to '%s' (%s)", arg, ar.name, extramsg);
}

/* the type error of every dialect: 5.3 on name the argument's type by its
   metatable's __name, and a light userdata as such */
LUNA_HIDDEN int luna_c_luaL_typeerror(lua_State *L, int arg, const char *tname) {
  const char *typearg;
  if (VNUM(L) >= 503 && luna_c_luaL_getmetafield(L, arg, "__name") == LUA_TSTRING)
    typearg = lua_tostring(L, -1);
  else if (VNUM(L) >= 503 && lua_type(L, arg) == LUA_TLIGHTUSERDATA)
    typearg = "light userdata";
  else
    typearg = luaL_typename(L, arg);
  return luna_c_luaL_argerror(L, arg,
                              luna_c_lua_pushfstring(L, "%s expected, got %s", tname, typearg));
}

/* 5.1's name of it */
LUNA_HIDDEN int luna_c_luaL_typerror(lua_State *L, int arg, const char *tname) {
  return luna_c_luaL_typeerror(L, arg, tname);
}

static void tag_error(lua_State *L, int arg, int tag) {
  luna_c_luaL_typeerror(L, arg, lua_typename(L, tag));
}

LUNA_HIDDEN void luna_c_luaL_checkstack(lua_State *L, int space, const char *msg) {
  int v = VNUM(L);
  if (!lua_checkstack(L, v == 502 ? space + LUA_MINSTACK : space)) {
    if (v == 501 || msg)
      luna_c_luaL_error(L, "stack overflow (%s)", msg);
    else
      luna_c_luaL_error(L, "stack overflow");
  }
}

LUNA_HIDDEN void luna_c_luaL_checktype(lua_State *L, int arg, int t) {
  if (lua_type(L, arg) != t) tag_error(L, arg, t);
}

LUNA_HIDDEN void luna_c_luaL_checkany(lua_State *L, int arg) {
  if (lua_type(L, arg) == LUA_TNONE) luna_c_luaL_argerror(L, arg, "value expected");
}

LUNA_HIDDEN const char *luna_c_luaL_checklstring(lua_State *L, int arg, size_t *len) {
  const char *s = lua_tolstring(L, arg, len);
  if (!s) tag_error(L, arg, LUA_TSTRING);
  return s;
}

LUNA_HIDDEN const char *luna_c_luaL_optlstring(lua_State *L, int arg, const char *def,
                                               size_t *len) {
  if (lua_isnoneornil(L, arg)) {
    if (len) *len = (def ? strlen(def) : 0);
    return def;
  }
  return luna_c_luaL_checklstring(L, arg, len);
}

LUNA_HIDDEN lua_Number luna_c_luaL_checknumber(lua_State *L, int arg) {
  int isnum;
  lua_Number d = lua_tonumberx(L, arg, &isnum);
  if (!isnum) tag_error(L, arg, LUA_TNUMBER);
  return d;
}

LUNA_HIDDEN lua_Number luna_c_luaL_optnumber(lua_State *L, int arg, lua_Number def) {
  return lua_isnoneornil(L, arg) ? def : luna_c_luaL_checknumber(L, arg);
}

LUNA_HIDDEN lua_Integer luna_c_luaL_checkinteger(lua_State *L, int arg) {
  int isnum;
  lua_Integer d = lua_tointegerx(L, arg, &isnum);
  if (!isnum) {
    if (VNUM(L) >= 503 && lua_isnumber(L, arg))
      luna_c_luaL_argerror(L, arg, "number has no integer representation");
    else
      tag_error(L, arg, LUA_TNUMBER);
  }
  return d;
}

LUNA_HIDDEN lua_Integer luna_c_luaL_optinteger(lua_State *L, int arg, lua_Integer def) {
  return lua_isnoneornil(L, arg) ? def : luna_c_luaL_checkinteger(L, arg);
}

/* 5.2: lua_Unsigned is 32 bits there */
unsigned lua_tounsignedx(lua_State *L, int idx, int *isnum);

LUNA_HIDDEN unsigned luna_c_luaL_checkunsigned(lua_State *L, int arg) {
  int isnum;
  unsigned d = lua_tounsignedx(L, arg, &isnum);
  if (!isnum) tag_error(L, arg, LUA_TNUMBER);
  return d;
}

LUNA_HIDDEN unsigned luna_c_luaL_optunsigned(lua_State *L, int arg, unsigned def) {
  return lua_isnoneornil(L, arg) ? def : luna_c_luaL_checkunsigned(L, arg);
}

LUNA_HIDDEN int luna_c_luaL_checkoption(lua_State *L, int arg, const char *def,
                                        const char *const lst[]) {
  const char *name = def ? luna_c_luaL_optlstring(L, arg, def, NULL)
                         : luna_c_luaL_checklstring(L, arg, NULL);
  int i;
  for (i = 0; lst[i]; i++)
    if (strcmp(lst[i], name) == 0) return i;
  return luna_c_luaL_argerror(L, arg, luna_c_lua_pushfstring(L, "invalid option '%s'", name));
}
