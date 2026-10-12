/* an allocation function with a byte limit, and a collector stopped by
   collectgarbage("stop"): a loop making far more garbage than the limit
   finishes only where an allocation that hits the limit runs an emergency
   collection first (5.3 on; 5.2 skips it while the collector is stopped,
   5.1 has none). */
#include "alloc_common.h"

static const char *script =
    "collectgarbage('stop')\n"
    "local n = 0\n"
    "for i = 1, 20000 do local s = string.rep('x', 200) .. i n = n + #s end\n"
    "return n\n";

int main(void) {
  char buf[96];
  int st;
  Ctl c;
  lua_State *L;

  memset(&c, 0, sizeof(c));
  L = NEWSTATE(ctl_alloc, &c);
  luaL_openlibs(L);
  if (luaL_loadstring(L, script) != 0) {
    printf("does not load: %s\n", lua_tostring(L, -1));
    return 1;
  }
  lua_gc(L, LUA_GCCOLLECT, 0);
  /* room for what the state holds now and about 1 MB more; the loop makes
     some 4 MB of strings */
  c.mode = LIMIT;
  c.limit = c.live + (1 << 20);
  st = pcall(L, 0, 1);
  outcome_of(L, st, buf, sizeof(buf));
  printf("garbage past the limit: %s\n", buf);
  lua_close(L);
  printf("every block freed after lua_close: %s\n", c.blocks == 0 && c.live == 0 ? "yes" : "no");
  return 0;
}
