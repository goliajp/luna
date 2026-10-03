/*
 * 5.1's luaL_Buffer: a fixed array, emptied onto the stack as a string
 * whenever it fills; the pieces on the stack are joined as they pile up
 * and once more by luaL_pushresult. The 5.1 headers name these functions
 * luna_*_51, as the layout differs from later versions'.
 */
#include <string.h>
#include "aux.h"

#define BUFFERSIZE BUFSIZ
#define LIMIT (LUA_MINSTACK / 2)

typedef struct luaL_Buffer51 {
  char *p;
  int lvl;
  lua_State *L;
  char buffer[BUFFERSIZE];
} luaL_Buffer51;

#define bufflen(B) ((size_t)((B)->p - (B)->buffer))
#define bufffree(B) ((size_t)(BUFFERSIZE - bufflen(B)))

static int emptybuffer(luaL_Buffer51 *B) {
  size_t l = bufflen(B);
  if (l == 0) return 0;
  lua_pushlstring(B->L, B->buffer, l);
  B->p = B->buffer;
  B->lvl++;
  return 1;
}

static void adjuststack(luaL_Buffer51 *B) {
  if (B->lvl > 1) {
    lua_State *L = B->L;
    int toget = 1;
    size_t toplen = lua_objlen(L, -1);
    do {
      size_t l = lua_objlen(L, -(toget + 1));
      if (B->lvl - toget + 1 >= LIMIT || toplen > l) {
        toplen += l;
        toget++;
      }
      else
        break;
    } while (toget < B->lvl);
    lua_concat(L, toget);
    B->lvl = B->lvl - toget + 1;
  }
}

LUNA_HIDDEN char *luna_c_luna_prepbuffer_51(luaL_Buffer51 *B) {
  if (emptybuffer(B)) adjuststack(B);
  return B->buffer;
}

LUNA_HIDDEN void luna_c_luna_addlstring_51(luaL_Buffer51 *B, const char *s, size_t l) {
  while (l--) {
    if (!(B->p < B->buffer + BUFFERSIZE)) luna_c_luna_prepbuffer_51(B);
    *B->p++ = *s++;
  }
}

LUNA_HIDDEN void luna_c_luna_addstring_51(luaL_Buffer51 *B, const char *s) {
  luna_c_luna_addlstring_51(B, s, strlen(s));
}

LUNA_HIDDEN void luna_c_luna_pushresult_51(luaL_Buffer51 *B) {
  emptybuffer(B);
  lua_concat(B->L, B->lvl);
  B->lvl = 1;
}

LUNA_HIDDEN void luna_c_luna_addvalue_51(luaL_Buffer51 *B) {
  lua_State *L = B->L;
  size_t vl;
  const char *s = lua_tolstring(L, -1, &vl);
  if (vl <= bufffree(B)) {
    memcpy(B->p, s, vl);
    B->p += vl;
    lua_pop(L, 1);
  }
  else {
    if (emptybuffer(B)) lua_insert(L, -2);
    B->lvl++;
    adjuststack(B);
  }
}

LUNA_HIDDEN void luna_c_luna_buffinit_51(lua_State *L, luaL_Buffer51 *B) {
  B->L = L;
  B->p = B->buffer;
  B->lvl = 0;
}
