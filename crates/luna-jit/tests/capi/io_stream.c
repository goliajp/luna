/* file handles seen from C: luaL_checkudata(L, i, LUA_FILEHANDLE) gives
   the stream (5.1: a FILE *) of io.stdout and of an opened file, and a
   stream C makes with its own close function works with the io methods */
#include <stdio.h>
#include <string.h>
#include "lua.h"
#include "lauxlib.h"
#include "lualib.h"

#if LUA_VERSION_NUM == 501
#define STREAM_FILE(L, i) (*(FILE **)luaL_checkudata(L, i, LUA_FILEHANDLE))
#else
#define STREAM_FILE(L, i) (((luaL_Stream *)luaL_checkudata(L, i, LUA_FILEHANDLE))->f)
#endif

/* which FILE * a handle holds */
static int which(lua_State *L) {
  FILE *f = STREAM_FILE(L, 1);
  lua_pushstring(L, f == stdout ? "stdout" : f == stdin ? "stdin" : f == stderr ? "stderr"
                                          : f == NULL ? "null" : "other");
  return 1;
}

/* write through the handle's FILE * from C */
static int cwrite(lua_State *L) {
  FILE *f = STREAM_FILE(L, 1);
  fputs(luaL_checkstring(L, 2), f);
  return 0;
}

#if LUA_VERSION_NUM >= 502
static int closes;

static int my_close(lua_State *L) {
  luaL_Stream *p = (luaL_Stream *)luaL_checkudata(L, 1, LUA_FILEHANDLE);
  closes++;
  printf("my_close: %d\n", fclose(p->f));
  lua_pushboolean(L, 1);
  return 1;
}

/* a stream over a file C opens, as io libraries outside liolib make them */
static int cstream(lua_State *L) {
  const char *name = luaL_checkstring(L, 1);
#if LUA_VERSION_NUM >= 504
  luaL_Stream *p = (luaL_Stream *)lua_newuserdatauv(L, sizeof(luaL_Stream), 0);
#else
  luaL_Stream *p = (luaL_Stream *)lua_newuserdata(L, sizeof(luaL_Stream));
#endif
  p->closef = NULL;
  luaL_setmetatable(L, LUA_FILEHANDLE);
  p->f = fopen(name, "w+");
  p->closef = my_close;
  return 1;
}
#endif

static const char *script =
  "print(which(io.stdout), which(io.stdin), which(io.stderr))\n"
  "local name = 'luna_io_stream.tmp'\n"
  "local f = io.open(name, 'w')\n"
  "print(which(f))\n"
  "cwrite(f, 'from C ') f:write('from Lua')\n"
  "f:close()\n"
  "print(pcall(function() return which(42) end))\n"
  "print(io.open(name):read('*a'))\n"
  "io.stdout:write('stdout via io ') cwrite(io.stdout, 'stdout via C\\n')\n"
  "if cstream then\n"
  "  local s = cstream(name)\n"
  "  print(io.type(s)) s:write('c stream') s:seek('set') print(s:read('*a'))\n"
  "  print(s:close()) print(io.type(s))\n"
  "  local t = cstream(name) t = nil collectgarbage() collectgarbage()\n"
  "end\n"
  "os.remove(name)\n"
  "if getfenv then\n"
  "  local e = getfenv(io.read)\n"
  "  print(type(e), e[1] == io.stdin, e[2] == io.stdout, debug.getfenv(io.stdout) == debug.getfenv(io.stderr))\n"
  "  print(getfenv(io.popen).__close ~= e.__close, getfenv(io.open) == e)\n"
  "end\n"
  "LEFT = io.open('luna_io_left.tmp', 'w') LEFT:write('left open')\n";

int main(void) {
  lua_State *L = luaL_newstate();
  int st;
  luaL_openlibs(L);
  lua_register(L, "which", which);
  lua_register(L, "cwrite", cwrite);
#if LUA_VERSION_NUM >= 502
  lua_register(L, "cstream", cstream);
#endif
  st = luaL_loadstring(L, script);
  if (st == 0) st = lua_pcall(L, 0, 0, 0);
  if (st) printf("error: %s\n", lua_tostring(L, -1));
#if LUA_VERSION_NUM >= 502
  printf("closes: %d\n", closes);
#endif
  fflush(stdout);
  lua_close(L);
  {
    /* the state's close finalized the file it left open */
    char buf[32] = {0};
    FILE *f = fopen("luna_io_left.tmp", "r");
    if (f) {
      fgets(buf, sizeof buf, f);
      fclose(f);
    }
    printf("after close: %s\n", buf);
    remove("luna_io_left.tmp");
  }
  return 0;
}
