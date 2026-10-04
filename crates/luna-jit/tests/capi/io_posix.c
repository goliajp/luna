/* io.popen on POSIX systems: reading a command's output, writing to its
   input, the status close returns, and the mode check (5.3 on) */
#include <stdio.h>
#include "lua.h"
#include "lauxlib.h"
#include "lualib.h"

static const char *script =
  "local p = io.popen('echo popen out; echo second')\n"
  "print(io.type(p), p:read('*l'), p:read('*a'))\n"
  "print(p:close())\n"
  "p = io.popen('exit 3')\n"
  "print(p:close())\n"
  "p = io.popen('cat', 'w')\n"
  "p:write('to cat\\n')\n"
  "print(p:close())\n"
  "print(pcall(io.popen, 'true', 'rw'))\n"
  "for l in io.popen('printf \"a\\\\nb\\\\n\"'):lines() do print('line', l) end\n";

int main(void) {
  lua_State *L = luaL_newstate();
  int st;
  luaL_openlibs(L);
  st = luaL_loadstring(L, script);
  if (st == 0) st = lua_pcall(L, 0, 0, 0);
  if (st) printf("error: %s\n", lua_tostring(L, -1));
  fflush(stdout);
  lua_close(L);
  return 0;
}
