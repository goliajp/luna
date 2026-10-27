/* luna_newstate_seeded and luna_newstate_with_seeded, which PUC does not
   have, and 5.5's lua_newstate with its seed: two states made with one seed
   iterate a table in the same order, and over seeds 1 to 16 the order is not
   always the same, except in 5.1, whose strings hash without a seed. */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "lua.h"
#include "lauxlib.h"
#include "lualib.h"

#define SEEDS 16

static const char *script =
    "local t = {}\n"
    "for i = 1, 40 do t['k' .. i] = i end\n"
    "local o = {}\n"
    "for k in pairs(t) do o[#o + 1] = k end\n"
    "return table.concat(o, ' ')\n";

static void *alloc(void *ud, void *p, size_t osize, size_t nsize) {
  (void)ud;
  (void)osize;
  if (nsize == 0) {
    free(p);
    return NULL;
  }
  return realloc(p, nsize);
}

/* the order the script sees in L, which is closed after */
static char *order(lua_State *L) {
  const char *s;
  char *copy;
  luaL_openlibs(L);
  if (luaL_loadstring(L, script) != 0 || lua_pcall(L, 0, 1, 0) != 0) {
    printf("error: %s\n", lua_tostring(L, -1));
    exit(1);
  }
  s = lua_tostring(L, -1);
  copy = malloc(strlen(s) + 1);
  strcpy(copy, s);
  lua_close(L);
  return copy;
}

static lua_State *aux_state(unsigned seed) {
  return luna_newstate_seeded(LUA_VERSION_NUM, seed);
}

static lua_State *host_state(unsigned seed) {
  return luna_newstate_with_seeded(LUA_VERSION_NUM, alloc, NULL, seed);
}

#if LUA_VERSION_NUM >= 505
static lua_State *newstate_55(unsigned seed) {
  return lua_newstate(alloc, NULL, seed);
}
#endif

static void check(const char *name, lua_State *(*make)(unsigned)) {
  char *first[SEEDS];
  int same = 1, several = 0;
  unsigned s;
  for (s = 0; s < SEEDS; s++) {
    char *again;
    first[s] = order(make(s + 1));
    again = order(make(s + 1));
    if (strcmp(first[s], again) != 0) same = 0;
    if (strcmp(first[s], first[0]) != 0) several = 1;
    free(again);
  }
  for (s = 0; s < SEEDS; s++) free(first[s]);
  printf("%s: one seed, one order: %s; several orders over seeds 1 to %d: %s\n",
         name, same ? "yes" : "no", SEEDS, several ? "yes" : "no");
}

int main(void) {
  check("luna_newstate_seeded", aux_state);
  check("luna_newstate_with_seeded", host_state);
#if LUA_VERSION_NUM >= 505
  check("lua_newstate", newstate_55);
#endif
  return 0;
}
