/* allocation functions: lua_newstate with the host's allocator (blocks it
   allocated are all freed by lua_close), one that fails, lua_getallocf /
   lua_setallocf, and luaL_newstate's allocator. */
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
  long live;
  int used;
} Count;

static void *counting(void *ud, void *ptr, size_t osize, size_t nsize) {
  Count *c = (Count *)ud;
  (void)osize;
  c->used = 1;
  if (nsize == 0) {
    if (ptr != NULL) c->live--;
    free(ptr);
    return NULL;
  }
  if (ptr == NULL) c->live++;
  return realloc(ptr, nsize);
}

static void *failing(void *ud, void *ptr, size_t osize, size_t nsize) {
  (void)ud;
  (void)osize;
  if (nsize == 0) {
    free(ptr);
    return NULL;
  }
  return NULL;
}

static void *other(void *ud, void *ptr, size_t osize, size_t nsize) {
  return counting(ud, ptr, osize, nsize);
}

int main(void) {
  Count c = {0, 0}, c2 = {0, 0};
  void *ud = NULL;
  lua_Alloc f;
  lua_State *L = NEWSTATE(counting, &c);
  printf("lua_newstate: %s, allocator used: %s\n", L ? "state" : "NULL", c.used ? "yes" : "no");
  f = lua_getallocf(L, &ud);
  printf("getallocf gives the function: %s, the user data: %s\n", f == counting ? "yes" : "no",
         ud == &c ? "yes" : "no");
  printf("getallocf with no ud: %s\n", lua_getallocf(L, NULL) == counting ? "yes" : "no");
  luaL_openlibs(L);
  luaL_loadstring(L, "local t = {} for i = 1, 1000 do t[i] = tostring(i) end return #t");
  lua_call(L, 0, 1);
  printf("ran: %d\n", (int)lua_tointeger(L, -1));
  lua_setallocf(L, other, &c);
  printf("setallocf: %s\n", lua_getallocf(L, &ud) == other && ud == &c ? "yes" : "no");
  lua_close(L);
  printf("after close, blocks still allocated: %ld\n", c.live);

  L = NEWSTATE(failing, NULL);
  printf("lua_newstate with a failing allocator: %s\n", L ? "state" : "NULL");

  L = luaL_newstate();
  f = lua_getallocf(L, &ud);
  printf("luaL_newstate has an allocator: %s\n", f != NULL ? "yes" : "no");
  {
    char *p = (char *)f(ud, NULL, 0, 16);
    p[0] = 'x';
    p = (char *)f(ud, p, 16, 64);
    printf("its blocks are usable: %c\n", p[0]);
    printf("freeing returns NULL: %s\n", f(ud, p, 64, 0) == NULL ? "yes" : "no");
  }
  lua_setallocf(L, counting, &c2);
  lua_close(L);
  printf("closed\n");
  return 0;
}
