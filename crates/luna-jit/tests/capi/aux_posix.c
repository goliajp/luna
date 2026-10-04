/* The auxiliary library's POSIX-only cases: a command killed by a signal
   (luaL_execresult decodes a wait status) and reading a directory, which
   Windows refuses to open. aux_meta and aux_load cover the rest on every
   platform. */
#include <errno.h>
#include <string.h>
#include "aux_common.h"

int main(void) {
  lua_State *L = luaL_newstate();
  int st;
  luaL_openlibs(L);
  st = luaL_loadfile(L, ".");
  printf("loadfile dir: status=%d read=%d detail=%d\n", st,
         strncmp(lua_tostring(L, -1), "cannot read .", 13) == 0, lua_tostring(L, -1)[13] == ':');
  lua_settop(L, 0);
#if LUA_VERSION_NUM >= 502
  errno = 0;
  luaL_execresult(L, 9);
  show_from(L, "execresult signal", 1);
  lua_settop(L, 0);
#endif
  lua_close(L);
  return 0;
}
