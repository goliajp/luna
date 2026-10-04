/* the status a thread that died keeps: lua_resume's result and
   lua_status for a runtime error, a memory error (the state's allocator
   refuses large blocks), an error in a finalizer during a collection
   (5.2/5.3), and a thread that returned; lua_pcall's status for the same
   memory error */
#include <stdlib.h>
#include "threads_common.h"

#if LUA_VERSION_NUM >= 505
#define NEWSTATE(f, ud) lua_newstate(f, ud, 0)
#else
#define NEWSTATE(f, ud) lua_newstate(f, ud)
#endif

/* refuses blocks over 64 MiB */
static void *limited(void *ud, void *ptr, size_t osize, size_t nsize) {
  (void)ud;
  (void)osize;
  if (nsize == 0) {
    free(ptr);
    return NULL;
  }
  if (nsize > ((size_t)1 << 26)) return NULL;
  return realloc(ptr, nsize);
}

/* a string between 1 GiB and 2 GiB: past the 64 MiB the allocator gives,
   short of the size string.rep refuses itself */
#define BIG "string.rep('x', 3 * 2^29)"

static void dies(lua_State *L, const char *tag, const char *body) {
  lua_State *co = lua_newthread(L);
  int nres, st;
  eval(co, body);
  st = resume(co, L, 0, &nres);
  /* what a dead thread keeps under the error is the dialect's own */
  printf("%s: status=%d lua_status=%d top: ", tag, st, lua_status(co));
  show_value(co, -1);
  printf("\n");
  printf("  again: lua_status=%d\n", lua_status(co));
  lua_pop(L, 1);
}

int main(void) {
  lua_State *L = NEWSTATE(limited, NULL);
  int st;
  luaL_openlibs(L);

  dies(L, "runtime error", "return function() error('boom', 0) end");
  dies(L, "memory error", "return function() local s = " BIG " return #s end");
  dies(L, "memory error caught inside",
       "return function() return pcall(string.rep, 'x', 3 * 2^29) end");
  dies(L, "returned", "return function() return 'done' end");
#if LUA_VERSION_NUM == 502 || LUA_VERSION_NUM == 503
  dies(L, "finalizer error",
       "return function()\n"
       "  setmetatable({}, {__gc = function() error('in gc', 0) end})\n"
       "  collectgarbage()\n"
       "  return 'not reached'\n"
       "end");
#endif

  /* from Lua: the same thread through coroutine.resume and status */
  run(L, "local co = coroutine.create(function() return " BIG " end)\n"
         "print('coroutine.resume', coroutine.resume(co))\n"
         "print('coroutine.status', coroutine.status(co))");

  /* lua_pcall reports the memory error's status */
  luaL_loadstring(L, "return " BIG);
  st = lua_pcall(L, 0, 1, 0);
  printf("pcall: status=%d %s\n", st, lua_tostring(L, -1));
  lua_pop(L, 1);
  lua_close(L);
  return 0;
}
