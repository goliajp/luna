/*
 * luaL_traceback and the search for a function's global name that it and
 * luaL_argerror share.
 */
#include <string.h>
#include "aux.h"

/* search the table on top for the value at objidx, two levels deep;
   pushes "name" or "lib.name" and returns 1 when found */
static int findfield(lua_State *L, int objidx, int level) {
  if (level == 0 || !lua_istable(L, -1)) return 0;
  lua_pushnil(L);
  while (lua_next(L, -2)) {
    if (lua_type(L, -2) == LUA_TSTRING) {
      if (lua_rawequal(L, objidx, -1)) {
        lua_pop(L, 1);
        return 1;
      }
      else if (findfield(L, objidx, level - 1)) {
        lua_pushliteral(L, ".");
        lua_replace(L, -3);
        lua_concat(L, 3);
        return 1;
      }
    }
    lua_pop(L, 1);
  }
  return 0;
}

/* 5.2 searches the globals; later versions the loaded modules, dropping
   a leading "_G." */
LUNA_HIDDEN int luna_aux_pushglobalfuncname(lua_State *L, aux_debug *d) {
  int top = lua_gettop(L);
  lua_getinfo(L, "f", (lua_Debug *)d);
  if (VNUM(L) == 502)
    luna_aux_pushglobaltable(L);
  else
    lua_getfield(L, REGIDX(L), "_LOADED");
  if (VNUM(L) >= 504) luna_c_luaL_checkstack(L, 6, "not enough stack");
  if (findfield(L, top + 1, 2)) {
    const char *name = lua_tostring(L, -1);
    if (VNUM(L) >= 503 && strncmp(name, "_G.", 3) == 0) {
      lua_pushstring(L, name + 3);
      lua_remove(L, -2);
    }
    lua_copy(L, -1, top + 1);
    lua_settop(L, top + 1);
    return 1;
  }
  lua_settop(L, top);
  return 0;
}

static int pushglobal(lua_State *L, aux_debug *d) {
  if (luna_aux_pushglobalfuncname(L, d)) {
    luna_c_lua_pushfstring(L, "function '%s'", lua_tostring(L, -1));
    lua_remove(L, -2);
    return 1;
  }
  return 0;
}

static void pushfuncname(lua_State *L, aux_debug *d, struct aux_ar *ar) {
  int v = VNUM(L);
  if (v == 502) {
    if (*ar->namewhat != '\0')
      luna_c_lua_pushfstring(L, "function '%s'", ar->name);
    else if (*ar->what == 'm')
      lua_pushliteral(L, "main chunk");
    else if (*ar->what == 'C') {
      if (!pushglobal(L, d)) lua_pushliteral(L, "?");
    }
    else
      luna_c_lua_pushfstring(L, "function <%s:%d>", ar->short_src, ar->linedefined);
    return;
  }
  if (v <= 504 && pushglobal(L, d))
    return;
  if (*ar->namewhat != '\0')
    luna_c_lua_pushfstring(L, "%s '%s'", ar->namewhat, ar->name);
  else if (*ar->what == 'm')
    lua_pushliteral(L, "main chunk");
  else if (v >= 505 && pushglobal(L, d))
    return;
  else if (*ar->what != 'C')
    luna_c_lua_pushfstring(L, "function <%s:%d>", ar->short_src, ar->linedefined);
  else
    lua_pushliteral(L, "?");
}

static int lastlevel(lua_State *L) {
  aux_debug d;
  int li = 1, le = 1;
  while (lua_getstack(L, le, (lua_Debug *)&d)) {
    li = le;
    le *= 2;
  }
  while (li < le) {
    int m = (li + le) / 2;
    if (lua_getstack(L, m, (lua_Debug *)&d))
      li = m + 1;
    else
      le = m;
  }
  return le - 1;
}

/* the line of one level: where it is, then its function's name */
static void pushlevel(lua_State *L, aux_debug *d, struct aux_ar *ar) {
  if (ar->currentline <= 0)
    luna_c_lua_pushfstring(L, "\n\t%s: in ", ar->short_src);
  else
    luna_c_lua_pushfstring(L, "\n\t%s:%d: in ", ar->short_src, ar->currentline);
  pushfuncname(L, d, ar);
  if (ar->istailcall) lua_pushliteral(L, "\n\t(...tail calls...)");
}

LUNA_HIDDEN void luna_c_luaL_traceback(lua_State *L, lua_State *L1, const char *msg,
                                       int level) {
  aux_debug d;
  struct aux_ar ar;
  int v = VNUM(L);
  int top = lua_gettop(L);
  int last = lastlevel(L1);
  int levels1 = v == 502 ? 12 : 10, levels2 = v == 502 ? 10 : 11;
  /* 5.2 counts the levels the way 5.3 on count the last one */
  int limit = v == 502 ? (last > levels1 + levels2 ? levels1 : -1)
                       : (last - level > levels1 + levels2 ? levels1 : -1);
  if (msg) luna_c_lua_pushfstring(L, "%s\n", msg);
  if (v == 503) luna_c_luaL_checkstack(L, 10, NULL);
  lua_pushliteral(L, "stack traceback:");
  while (lua_getstack(L1, level++, (lua_Debug *)&d)) {
    if (v == 502 ? level == limit : limit-- == 0) {
      if (v <= 503) {
        lua_pushliteral(L, "\n\t...");
        level = v == 502 ? last - levels2 : last - levels2 + 1;
      }
      else {
        int n = last - level - levels2 + 1;
        luna_c_lua_pushfstring(L, "\n\t...\t(skipping %d levels)", n);
        level += n;
      }
    }
    else {
      luna_aux_getinfo(L1, "Slnt", &d, &ar);
      pushlevel(L, &d, &ar);
    }
    lua_concat(L, lua_gettop(L) - top);
  }
  lua_concat(L, lua_gettop(L) - top);
}
