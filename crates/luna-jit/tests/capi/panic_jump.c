/* a panic function that jumps out: the process goes on */
#include <setjmp.h>
#include <stdio.h>
#include "lua.h"
#include "lauxlib.h"

static jmp_buf escape;

static int jumper(lua_State *L) {
  printf("panic: %s\n", lua_tostring(L, -1));
  longjmp(escape, 1);
  return 0;
}

int main(void) {
  lua_State *L = luaL_newstate();
  lua_atpanic(L, jumper);
  if (setjmp(escape) == 0) {
    lua_pushstring(L, "jump out");
    lua_error(L);
    printf("NOT REACHED\n");
  } else {
    printf("back in main\n");
  }
  return 0;
}
