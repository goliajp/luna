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
 *
 * With HOST_ALLOC_LOG naming a file, every call of the allocator is
 * written there (`a <tag or old size> <size>`, `r <old> <new>`, `d <old>`),
 * with `M <mark>` lines around the second luaL_openlibs and the collection
 */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "lua.h"
#include "lauxlib.h"
#include "lualib.h"

static FILE *alloc_log;

static void mark(const char *name) {
  if (alloc_log) fprintf(alloc_log, "M %s\n", name);
}

/* luaL_newstate's allocator, logging */
static void *logged_alloc(void *ud, void *ptr, size_t osize, size_t nsize) {
  (void)ud;
  if (ptr == NULL) {
    if (nsize > 0) fprintf(alloc_log, "a %lu %lu\n", (unsigned long)osize, (unsigned long)nsize);
  } else if (nsize == 0) {
    fprintf(alloc_log, "d %lu\n", (unsigned long)osize);
  } else {
    fprintf(alloc_log, "r %lu %lu\n", (unsigned long)osize, (unsigned long)nsize);
  }
  if (nsize == 0) {
    free(ptr);
    return NULL;
  }
  return realloc(ptr, nsize);
}

static int panic(lua_State *L) {
  fprintf(stderr, "PANIC: %s\n", lua_tostring(L, -1));
  return 0;
}

static lua_State *newstate(void) {
  const char *log = getenv("HOST_ALLOC_LOG");
  lua_State *L;
  if (log == NULL) return luaL_newstate();
  alloc_log = fopen(log, "w");
#if LUA_VERSION_NUM >= 505
  L = lua_newstate(logged_alloc, NULL, luaL_makeseed(NULL));
#else
  L = lua_newstate(logged_alloc, NULL);
#endif
  if (L) lua_atpanic(L, &panic);
  return L;
}

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
  lua_State *L = newstate();
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
  mark("reopen");
  if (strcmp(argv[1], "reopen") == 0) luaL_openlibs(L);
  mark("/reopen");
  after = count(L);
  mark("collect");
  lua_gc(L, LUA_GCCOLLECT, 0);
  mark("/collect");
  printf("bytes %ld collected %ld\n", after - before, count(L) - before);
  lua_close(L);
  return 0;
}
