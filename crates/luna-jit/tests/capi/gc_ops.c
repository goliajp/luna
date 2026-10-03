/* lua_gc: every option of the dialect with its numbering and results,
   parameters written and read back, options the dialect does not have,
   lua_gc inside a finalizer, and finalizer errors of a full collection. */
#include <stdio.h>
#include "lua.h"
#include "lauxlib.h"
#include "lualib.h"

#if LUA_VERSION_NUM >= 504
#define GC1(L, w, d) lua_gc(L, w, d)
#define GC0(L, w) lua_gc(L, w)
#else
#define GC1(L, w, d) lua_gc(L, w, d)
#define GC0(L, w) lua_gc(L, w, 0)
#endif

static int gc_in_finalizer(lua_State *L) {
  int count = GC0(L, LUA_GCCOUNT);
  int coll = GC0(L, LUA_GCCOLLECT);
  printf("  in a finalizer: count %s, collect %d\n", count < 0 ? "-1" : ">= 0", coll);
  return 0;
}

static int collect_in_c(lua_State *L) {
  int r = GC0(L, LUA_GCCOLLECT);
  printf("  collect returned %d\n", r);
  return 0;
}

static void run(lua_State *L, const char *src) {
  luaL_loadstring(L, src);
  lua_call(L, 0, 0);
}

int main(void) {
  lua_State *L = luaL_newstate();
  int i, r, prev;
  luaL_openlibs(L);

  printf("stop %d\n", GC0(L, LUA_GCSTOP));
#if LUA_VERSION_NUM >= 502
  printf("  running: %d\n", GC0(L, LUA_GCISRUNNING));
#endif
  printf("restart %d\n", GC0(L, LUA_GCRESTART));
#if LUA_VERSION_NUM >= 502
  printf("  running: %d\n", GC0(L, LUA_GCISRUNNING));
#endif
  printf("collect %d\n", GC0(L, LUA_GCCOLLECT));
  r = GC0(L, LUA_GCCOUNT);
  printf("count is positive: %s\n", r > 0 ? "yes" : "no");
  r = GC0(L, LUA_GCCOUNTB);
  printf("countb is below 1024: %s\n", r >= 0 && r < 1024 ? "yes" : "no");
  run(L, "garbage = {} for i = 1, 2000 do garbage[i] = {i} end garbage = nil");
  r = GC1(L, LUA_GCSTEP, 0);
  printf("a basic step returns 0 or 1: %s\n", r == 0 || r == 1 ? "yes" : "no");
  for (i = 0; i < 100000 && GC1(L, LUA_GCSTEP, 0) != 1; i++) {}
  printf("basic steps finish a cycle: %s\n", i < 100000 ? "yes" : "no");
  for (i = 0; i < 10000 && GC1(L, LUA_GCSTEP, 100) != 1; i++) {}
  printf("steps of 100 finish a cycle: %s\n", i < 10000 ? "yes" : "no");
  GC0(L, LUA_GCSTOP);
  for (i = 0; i < 100000 && GC1(L, LUA_GCSTEP, 0) != 1; i++) {}
  printf("steps run while stopped: %s\n", i < 100000 ? "yes" : "no");
#if LUA_VERSION_NUM >= 502
  printf("  still stopped: %d\n", GC0(L, LUA_GCISRUNNING) == 0);
#endif
  GC0(L, LUA_GCRESTART);

#if LUA_VERSION_NUM <= 504
  printf("setpause: previous %d", GC1(L, LUA_GCSETPAUSE, 100));
  printf(", then %d", GC1(L, LUA_GCSETPAUSE, 150));
  printf(", then %d", GC1(L, LUA_GCSETPAUSE, 1000));
  printf(", then %d\n", GC1(L, LUA_GCSETPAUSE, 200));
  printf("setstepmul: previous %d", GC1(L, LUA_GCSETSTEPMUL, 10));
  printf(", then %d", GC1(L, LUA_GCSETSTEPMUL, 0));
  printf(", then %d", GC1(L, LUA_GCSETSTEPMUL, 401));
  prev = GC1(L, LUA_GCSETSTEPMUL, 200);
  printf(", then %d\n", prev);
#endif
#if LUA_VERSION_NUM == 502
  printf("setmajorinc: previous %d", GC1(L, LUA_GCSETMAJORINC, 50));
  printf(", then %d\n", GC1(L, LUA_GCSETMAJORINC, 200));
  printf("gen %d, inc %d, gen %d, inc %d\n", GC0(L, LUA_GCGEN), GC0(L, LUA_GCINC),
         GC0(L, LUA_GCGEN), GC0(L, LUA_GCINC));
  printf("isrunning after the switches: %d\n", GC0(L, LUA_GCISRUNNING));
#endif
#if LUA_VERSION_NUM == 504
  printf("gen: previous mode %d\n", lua_gc(L, LUA_GCGEN, 0, 0));
  printf("gen with params: previous mode %d\n", lua_gc(L, LUA_GCGEN, 30, 300));
  printf("inc: previous mode %d\n", lua_gc(L, LUA_GCINC, 0, 0, 0));
  printf("inc with params: previous mode %d\n", lua_gc(L, LUA_GCINC, 120, 300, 10));
  printf("pause now %d, stepmul now %d\n", lua_gc(L, LUA_GCSETPAUSE, 200),
         lua_gc(L, LUA_GCSETSTEPMUL, 100));
  printf("inc again: previous mode %d\n", lua_gc(L, LUA_GCINC, 0, 0, 0));
  run(L, "print('collectgarbage generational ->', collectgarbage('generational'))");
  printf("inc: previous mode %d\n", lua_gc(L, LUA_GCINC, 0, 0, 0));
  printf("setcstacklimit %d\n", lua_setcstacklimit(L, 1000));
#endif
#if LUA_VERSION_NUM >= 505
  printf("gen: previous mode %d\n", lua_gc(L, LUA_GCGEN));
  printf("gen: previous mode %d\n", lua_gc(L, LUA_GCGEN));
  printf("inc: previous mode %d\n", lua_gc(L, LUA_GCINC));
  printf("inc: previous mode %d\n", lua_gc(L, LUA_GCINC));
  for (i = 0; i < LUA_GCPN; i++)
    printf("param %d: %d\n", i, lua_gc(L, LUA_GCPARAM, i, -1));
  printf("set pause 123: previous %d", lua_gc(L, LUA_GCPARAM, LUA_GCPPAUSE, 123));
  printf(", now %d\n", lua_gc(L, LUA_GCPARAM, LUA_GCPPAUSE, -1));
  printf("set stepsize 0: previous %d", lua_gc(L, LUA_GCPARAM, LUA_GCPSTEPSIZE, 0));
  printf(", now %d\n", lua_gc(L, LUA_GCPARAM, LUA_GCPSTEPSIZE, -1));
  printf("set minormul 1000000: previous %d", lua_gc(L, LUA_GCPARAM, LUA_GCPMINORMUL, 1000000));
  printf(", now %d\n", lua_gc(L, LUA_GCPARAM, LUA_GCPMINORMUL, -1));
  r = lua_gc(L, LUA_GCSTEP, (size_t)0);
  printf("a step of size_t 0 returns 0 or 1: %s\n", r == 0 || r == 1 ? "yes" : "no");
  r = lua_gc(L, LUA_GCSTEP, (size_t)-1);
  printf("a step of a huge size_t returns 0 or 1: %s\n", r == 0 || r == 1 ? "yes" : "no");
  lua_gc(L, LUA_GCPARAM, LUA_GCPSTEPSIZE, 9600);
  run(L, "print('collectgarbage generational ->', collectgarbage('generational'))");
  printf("inc: previous mode %d\n", lua_gc(L, LUA_GCINC));
#endif
  (void)prev;

  printf("options the dialect does not have\n");
  for (i = -1; i <= 13; i++) {
    int known;
#if LUA_VERSION_NUM == 501
    known = i >= 0 && i <= 7;
#elif LUA_VERSION_NUM == 502
    known = i >= 0 && i <= 11;
#elif LUA_VERSION_NUM == 503
    known = (i >= 0 && i <= 7) || i == 9;
#elif LUA_VERSION_NUM == 504
    known = (i >= 0 && i <= 7) || i == 9 || i == 10 || i == 11;
#else
    known = i >= 0 && i <= 9;
#endif
    if (!known)
      printf("  option %d: %d\n", i, GC1(L, i, 0));
  }

  printf("lua_gc inside a finalizer\n");
  lua_pushcfunction(L, gc_in_finalizer);
  lua_setglobal(L, "gc_in_finalizer");
#if LUA_VERSION_NUM == 501
  run(L, "local u = newproxy(true) getmetatable(u).__gc = gc_in_finalizer u = nil");
#else
  run(L, "setmetatable({}, {__gc = gc_in_finalizer})");
#endif
  GC0(L, LUA_GCCOLLECT);
  printf("  after: count is positive: %s\n", GC0(L, LUA_GCCOUNT) > 0 ? "yes" : "no");

  printf("a full collection with a failing finalizer\n");
#if LUA_VERSION_NUM == 501
  run(L, "local u = newproxy(true) getmetatable(u).__gc = function() error('gc fail', 0) end u = nil");
#else
  run(L, "setmetatable({}, {__gc = function() error('gc fail', 0) end})");
#endif
  lua_pushcfunction(L, collect_in_c);
  r = lua_pcall(L, 0, 0, 0);
  printf("  pcall status %d", r);
  if (r != 0)
    printf(": %s", lua_tostring(L, -1));
  printf("\n");
  lua_settop(L, 0);

  lua_close(L);
  return 0;
}
