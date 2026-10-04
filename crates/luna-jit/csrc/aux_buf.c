/*
 * luaL_Buffer. 5.1's buffer keeps its pieces on the stack (the luna_*_51
 * functions); 5.2 on share one layout, where the content outgrows the
 * buffer into a userdata on the stack: a new one per growth in 5.2, a box
 * resized in place from 5.3, which 5.4 puts where a placeholder pushed by
 * luaL_buffinit was and marks to be closed. 5.5 hands the final content
 * to Lua as an external string.
 */
#include <stdint.h>
#include <string.h>
#include "auxlib.h"

/* 5.2 on; `init` starts at the same offset in every version's layout */
typedef struct luaL_Buffer {
  char *b;
  size_t size;
  size_t n;
  lua_State *L;
  char init[1];
} luaL_Buffer;

#define buffonstack(B) ((B)->b != (B)->init)

/* each version's LUAL_BUFFERSIZE */
static size_t buffersize(lua_State *L) {
  switch (VNUM(L)) {
    case 501:
    case 502: return BUFSIZ;
    case 503: return 0x80 * sizeof(void *) * sizeof(lua_Integer);
    default: return 16 * sizeof(void *) * sizeof(lua_Number);
  }
}

typedef struct UBox {
  void *box;
  size_t bsize;
} UBox;

static void *resizebox(lua_State *L, int idx, size_t newsize) {
  void *ud;
  lua_Alloc allocf = lua_getallocf(L, &ud);
  UBox *box = (UBox *)lua_touserdata(L, idx);
  void *temp;
  if (VNUM(L) >= 505 && box->bsize == newsize) return box->box;
  temp = allocf(ud, box->box, box->bsize, newsize);
  if (temp == NULL && newsize > 0) {
    if (VNUM(L) == 503) {
      resizebox(L, idx, 0);
      luna_c_luaL_error(L, "not enough memory for buffer allocation");
    }
    lua_pushliteral(L, "not enough memory");
    lua_error(L);
  }
  box->box = temp;
  box->bsize = newsize;
  return temp;
}

static int boxgc(lua_State *L) {
  resizebox(L, 1, 0);
  return 0;
}

static const luaL_Reg boxmt[] = {{"__gc", boxgc}, {"__close", boxgc}, {NULL, NULL}};

/* push a new empty box with its metatable: "LUABOX" (5.3) or "_UBOX*" */
static void newbox(lua_State *L) {
  UBox *box = (UBox *)lua_newuserdatauv(L, sizeof(UBox), VNUM(L) >= 504 ? 0 : 1);
  box->box = NULL;
  box->bsize = 0;
  if (VNUM(L) == 503) {
    if (luna_c_luaL_newmetatable(L, "LUABOX")) {
      lua_pushcfunction(L, boxgc);
      lua_setfield(L, -2, "__gc");
    }
  }
  else if (lua_getfield(L, REGIDX(L), "_UBOX*") == LUA_TNIL) {
    lua_createtable(L, 0, 2);
    luna_c_luaL_setfuncs(L, boxmt, 0);
    lua_copy(L, -1, -2);
    lua_setfield(L, REGIDX(L), "_UBOX*");
  }
  lua_setmetatable(L, -2);
}

static size_t newbuffsize(luaL_Buffer *B, size_t sz) {
  lua_State *L = B->L;
  size_t newsize;
  if (VNUM(L) <= 503) {
    newsize = B->size * 2;
    if (newsize - B->n < sz) newsize = B->n + sz;
    if (newsize < B->n || newsize - B->n < sz) luna_c_luaL_error(L, "buffer too large");
  }
  else if (VNUM(L) == 504) {
    newsize = (B->size / 2) * 3;
    if (SIZE_MAX - sz < B->n) return (size_t)luna_c_luaL_error(L, "buffer too large");
    if (newsize < B->n + sz) newsize = B->n + sz;
  }
  else {
    size_t max = (size_t)INT64_MAX < SIZE_MAX ? (size_t)INT64_MAX : SIZE_MAX;
    newsize = B->size;
    if (sz >= max - B->n) return (size_t)luna_c_luaL_error(L, "resulting string too large");
    if (newsize <= max / 3 * 2) newsize += newsize >> 1;
    if (newsize < B->n + sz + 1) newsize = B->n + sz + 1;
  }
  return newsize;
}

/* room for sz more bytes; the buffer's userdata (or 5.4's placeholder) is
   at boxidx */
static char *prepbuffsize(luaL_Buffer *B, size_t sz, int boxidx) {
  lua_State *L = B->L;
  int v = VNUM(L);
  char *newbuff;
  size_t newsize;
  if (B->size - B->n >= sz) return B->b + B->n;
  newsize = newbuffsize(B, sz);
  if (v <= 502) {
    newbuff = (char *)lua_newuserdatauv(L, newsize, 1);
    memcpy(newbuff, B->b, B->n);
    if (buffonstack(B)) lua_remove(L, -2);
  }
  else if (buffonstack(B))
    newbuff = (char *)resizebox(L, boxidx, newsize);
  else if (v == 503) {
    newbox(L);
    newbuff = (char *)resizebox(L, -1, newsize);
    memcpy(newbuff, B->b, B->n);
  }
  else {
    lua_remove(L, boxidx);
    newbox(L);
    lua_insert(L, boxidx);
    lua_toclose(L, boxidx);
    newbuff = (char *)resizebox(L, boxidx, newsize);
    memcpy(newbuff, B->b, B->n);
  }
  B->b = newbuff;
  B->size = newsize;
  return newbuff + B->n;
}

LUNA_HIDDEN char *luna_c_luaL_prepbuffsize(luaL_Buffer *B, size_t sz) {
  return prepbuffsize(B, sz, -1);
}

LUNA_HIDDEN char *luna_c_luaL_prepbuffer(luaL_Buffer *B) {
  return prepbuffsize(B, buffersize(B->L), -1);
}

LUNA_HIDDEN void luna_c_luaL_addlstring(luaL_Buffer *B, const char *s, size_t l) {
  if (l > 0 || VNUM(B->L) == 502) {
    char *b = prepbuffsize(B, l, -1);
    memcpy(b, s, l);
    B->n += l;
  }
}

LUNA_HIDDEN void luna_c_luaL_addstring(luaL_Buffer *B, const char *s) {
  luna_c_luaL_addlstring(B, s, strlen(s));
}

LUNA_HIDDEN void luna_c_luaL_pushresult(luaL_Buffer *B) {
  lua_State *L = B->L;
  int v = VNUM(L);
  if (v <= 503) {
    lua_pushlstring(L, B->b, B->n);
    if (buffonstack(B)) {
      if (v == 503) resizebox(L, -2, 0);
      lua_remove(L, -2);
    }
    return;
  }
  if (v == 504 || !buffonstack(B)) {
    lua_pushlstring(L, B->b, B->n);
    if (buffonstack(B)) lua_closeslot(L, -2);
  }
  else {
    UBox *box = (UBox *)lua_touserdata(L, -1);
    void *ud;
    lua_Alloc allocf = lua_getallocf(L, &ud);
    size_t len = B->n;
    char *s;
    resizebox(L, -1, len + 1);
    s = (char *)box->box;
    s[len] = '\0';
    box->bsize = 0;
    box->box = NULL;
    lua_pushexternalstring(L, s, len, allocf, ud);
    lua_closeslot(L, -2);
    lua_gc(L, LUA_GCSTEP, (int)len);
  }
  lua_remove(L, -2);
}

LUNA_HIDDEN void luna_c_luaL_pushresultsize(luaL_Buffer *B, size_t sz) {
  B->n += sz;
  luna_c_luaL_pushresult(B);
}

/* the value to add is on top, above the buffer's userdata if any */
LUNA_HIDDEN void luna_c_luaL_addvalue(luaL_Buffer *B) {
  lua_State *L = B->L;
  size_t l;
  const char *s = lua_tolstring(L, -1, &l);
  if (VNUM(L) <= 503) {
    if (buffonstack(B)) lua_insert(L, -2);
    luna_c_luaL_addlstring(B, s, l);
    lua_remove(L, buffonstack(B) ? -2 : -1);
  }
  else {
    char *b = prepbuffsize(B, l, -2);
    memcpy(b, s, l);
    B->n += l;
    lua_pop(L, 1);
  }
}

LUNA_HIDDEN void luna_c_luaL_buffinit(lua_State *L, luaL_Buffer *B) {
  B->L = L;
  B->b = B->init;
  B->n = 0;
  B->size = buffersize(L);
  if (VNUM(L) >= 504) lua_pushlightuserdata(L, (void *)B);
}

LUNA_HIDDEN char *luna_c_luaL_buffinitsize(lua_State *L, luaL_Buffer *B, size_t sz) {
  luna_c_luaL_buffinit(L, B);
  return prepbuffsize(B, sz, -1);
}

LUNA_HIDDEN void luna_c_luaL_addgsub(luaL_Buffer *b, const char *s, const char *p,
                                     const char *r) {
  const char *wild;
  size_t l = strlen(p);
  while ((wild = strstr(s, p)) != NULL) {
    luna_c_luaL_addlstring(b, s, (size_t)(wild - s));
    luna_c_luaL_addstring(b, r);
    s = wild + l;
  }
  luna_c_luaL_addstring(b, s);
}

/* a buffer with room for any version's initial size */
struct bigbuffer {
  luaL_Buffer b;
  char space[16384 + BUFSIZ];
};

LUNA_HIDDEN const char *luna_c_luaL_gsub(lua_State *L, const char *s, const char *p,
                                         const char *r) {
  struct bigbuffer bb;
  luna_c_luaL_buffinit(L, &bb.b);
  luna_c_luaL_addgsub(&bb.b, s, p, r);
  luna_c_luaL_pushresult(&bb.b);
  return lua_tostring(L, -1);
}
