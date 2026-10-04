/* every block of a state comes from its allocation function: the kinds of
   object new blocks are for (the old size of a new block), lua_gc's count
   against the bytes the function has handed out, and nothing left after
   lua_close, also across lua_setallocf. */
#include <stdio.h>
#include <stdlib.h>
#include "lua.h"
#include "lauxlib.h"
#include "lualib.h"

#if LUA_VERSION_NUM >= 505
#define NEWSTATE(f, ud) lua_newstate(f, ud, 0)
#else
#define NEWSTATE(f, ud) lua_newstate(f, ud)
#endif

typedef struct {
  size_t live;
  long blocks;
  int kinds[16];
} Count;

static void *counting(void *ud, void *ptr, size_t osize, size_t nsize) {
  Count *c = (Count *)ud;
  void *q;
  if (nsize == 0) {
    if (ptr != NULL) {
      c->live -= osize;
      c->blocks--;
    }
    free(ptr);
    return NULL;
  }
  q = realloc(ptr, nsize);
  if (q == NULL) return NULL;
  if (ptr == NULL) {
    c->blocks++;
    c->kinds[osize < 16 ? osize : 15] = 1;
    c->live += nsize;
  } else {
    c->live = c->live - osize + nsize;
  }
  return q;
}

static void kinds(const char *what, Count *c) {
  int i;
  printf("%s:", what);
  for (i = 0; i < 16; i++)
    if (c->kinds[i]) printf(" %d", i);
  printf("\n");
}

static int count_matches(lua_State *L, Count *c) {
  size_t kb = (size_t)lua_gc(L, LUA_GCCOUNT, 0);
  size_t b = (size_t)lua_gc(L, LUA_GCCOUNTB, 0);
  return kb * 1024 + b == c->live;
}

static const char *script =
    "local t = {}\n"
    "for i = 1, 300 do t[i] = {name = 'k' .. i, f = function() return i end} end\n"
    "local co = coroutine.create(function(x) coroutine.yield(x .. '!') end)\n"
    "assert(coroutine.resume(co, string.rep('ab', 100)))\n"
    "local s = 0\n"
    "for k, v in pairs(t) do s = s + #v.name end\n"
    "return s\n";

int main(void) {
  Count c = {0, 0, {0}}, c2 = {0, 0, {0}};
  lua_State *L = NEWSTATE(counting, &c);
  kinds("new state", &c);
  printf("count matches the allocator: %s\n", count_matches(L, &c) ? "yes" : "no");
  luaL_openlibs(L);
  if (luaL_loadstring(L, script) || lua_pcall(L, 0, 1, 0)) {
    printf("error: %s\n", lua_tostring(L, -1));
    return 1;
  }
  printf("ran: %d\n", (int)lua_tointeger(L, -1));
  lua_pop(L, 1);
  kinds("after a script", &c);
  printf("count matches the allocator: %s\n", count_matches(L, &c) ? "yes" : "no");
  lua_gc(L, LUA_GCCOLLECT, 0);
  printf("count matches after a collection: %s\n", count_matches(L, &c) ? "yes" : "no");
  lua_setallocf(L, counting, &c2);
  luaL_loadstring(L, "local t = {} for i = 1, 100 do t[i] = {} end return #t");
  lua_call(L, 0, 1);
  printf("ran on the second function: %d\n", (int)lua_tointeger(L, -1));
  lua_close(L);
  printf("after close: %s\n",
         c.blocks + c2.blocks == 0 && c.live + c2.live == 0 ? "everything freed" : "blocks left");
  return 0;
}
