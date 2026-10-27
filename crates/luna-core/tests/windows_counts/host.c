/* A host whose collector counts the tests in `tests/it/open_running.rs`
 * and `tests/it/reopen_libs.rs` compare with on Windows, where PUC's
 * default package paths name the executable's directory.
 *
 *   host running <chunk>   luaL_openlibs with the collector running, the
 *                          count, then what <chunk> returns
 *   host reopen <setup>    the libraries opened as lua.c opens them, the
 *   host noreopen <setup>  collector stopped, <setup> run, then a second
 *                          luaL_openlibs (or none): the bytes it adds, and
 *                          the count after a full collection, both less
 *                          the count before it
 */
#include <stdio.h>
#include <string.h>
#include "lua.h"
#include "lauxlib.h"
#include "lualib.h"

static long count(lua_State *L) {
  return (long)lua_gc(L, LUA_GCCOUNT, 0) * 1024 + lua_gc(L, LUA_GCCOUNTB, 0);
}

/* `src` loaded as luaL_loadstring loads it and called; its first result
 * printed when `show` */
static int run(lua_State *L, const char *src, int show) {
  if (luaL_loadstring(L, src) != 0 || lua_pcall(L, 0, 1, 0) != 0) {
    printf("error: %s\n", lua_tostring(L, -1));
    return 1;
  }
  if (show) printf("%s\n", lua_tostring(L, -1));
  lua_pop(L, 1);
  return 0;
}

int main(int argc, char **argv) {
  lua_State *L = luaL_newstate();
  long before, after;
  int err;
  if (argc != 3) return 2;
  if (strcmp(argv[1], "running") == 0) {
    luaL_openlibs(L);
    printf("opened %ld\n", count(L));
    err = run(L, argv[2], 1);
    lua_close(L);
    return err;
  }
#if LUA_VERSION_NUM != 503
  lua_gc(L, LUA_GCSTOP, 0);
#endif
  luaL_openlibs(L);
  lua_gc(L, LUA_GCSTOP, 0);
  if (run(L, argv[2], 0)) return 1;
  before = count(L);
  if (strcmp(argv[1], "reopen") == 0) luaL_openlibs(L);
  after = count(L);
  lua_gc(L, LUA_GCCOLLECT, 0);
  printf("bytes %ld collected %ld\n", after - before, count(L) - before);
  lua_close(L);
  return 0;
}
