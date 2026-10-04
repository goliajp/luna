/* lua_closethread (lua_resetthread before 5.4.6) on the main thread: it
   runs the __close of the to-be-closed slots on the main thread's stack,
   newest first, empties the stack, and reports an error a __close raises;
   the state stays usable */
#include "threads_common.h"

#if LUA_VERSION_NUM >= 504

#if LUA_VERSION_NUM >= 505 || LUA_VERSION_RELEASE_NUM >= 50406
#define CLOSE(co, from) lua_closethread(co, from)
#else
#define CLOSE(co, from) lua_resetthread(co)
#endif

/* push a value whose __close prints its tag and the error it gets, or
   raises when `raise` is set, and mark it to be closed */
static void push_tbc(lua_State *L, const char *tag, int raise) {
  lua_pushfstring(L,
                  "return setmetatable({}, {__close = function(_, e)\n"
                  "  print('__close %s', e)\n"
                  "  if %d == 1 then error('close %s failed', 0) end\n"
                  "end})",
                  tag, raise, tag);
  eval(L, lua_tostring(L, -1));
  lua_remove(L, -2);
  lua_toclose(L, -1);
}

int main(void) {
  lua_State *L = luaL_newstate();
  int st;
  luaL_openlibs(L);

  lua_pushinteger(L, 1);
  push_tbc(L, "first", 0);
  lua_pushstring(L, "between");
  push_tbc(L, "second", 0);
  show_stack(L, "before");
  st = CLOSE(L, NULL);
  printf("close: %d lua_status=%d ", st, lua_status(L));
  show_stack(L, "after");

  push_tbc(L, "ok", 0);
  push_tbc(L, "raises", 1);
  st = CLOSE(L, NULL);
  printf("close with error: %d ", st);
  show_stack(L, "after");
  lua_settop(L, 0);

  st = CLOSE(L, NULL);
  printf("close empty: %d ", st);
  show_stack(L, "after");

  run(L, "print('still usable', 1 + 1)");
  lua_close(L);
  return 0;
}

#else
int main(void) {
  return 0;
}
#endif
