/* lua_newstate sets no panic function: an unprotected error ends the
   process without a message */
#include <stdio.h>
#include <stdlib.h>
#include "lua.h"

static void *alloc(void *ud, void *ptr, size_t osize, size_t nsize) {
  (void)ud;
  (void)osize;
  if (nsize == 0) {
    free(ptr);
    return NULL;
  }
  return realloc(ptr, nsize);
}

int main(void) {
#if LUA_VERSION_NUM >= 505
  lua_State *L = lua_newstate(alloc, NULL, 0);
#else
  lua_State *L = lua_newstate(alloc, NULL);
#endif
  printf("panic function: %s\n", lua_atpanic(L, NULL) == NULL ? "none" : "set");
  fflush(stdout);
  lua_pushstring(L, "silent");
  lua_error(L);
  printf("NOT REACHED\n");
  return 0;
}
