/* an allocation function that refuses only the Nth growth, swept over N:
   from 5.2 on the emergency collection runs and the allocation is tried
   again, which then succeeds, so the scripts end as they do with nothing
   refused; 5.1 has no emergency collection and reports the memory
   error. */
#include "alloc_common.h"

static const char *script =
    "local t = {}\n"
    "for i = 1, 60 do t[i] = {k = 'v' .. i, f = function() return i end} end\n"
    "local co = coroutine.wrap(function(a) local b = coroutine.yield(a .. '!') return b .. '?' end)\n"
    "local x = co('a') .. co('b')\n"
    "local s = string.rep('ab', 500) .. table.concat(t[1], ',')\n"
    "return #t .. ' ' .. x .. ' ' .. t[60].f() .. ' ' .. #s\n";

int main(void) {
  Outcomes o;
  char buf[96];
  long n;
  int done, all_freed = 1;
  Ctl c;

  memset(&o, 0, sizeof(o));
  for (n = 0, done = 0; n < 200000 && !done; n++) {
    lua_State *L;
    int st;
    memset(&c, 0, sizeof(c));
    L = NEWSTATE(ctl_alloc, &c);
    luaL_openlibs(L);
    if (luaL_loadstring(L, script) != 0) {
      printf("does not load: %s\n", lua_tostring(L, -1));
      return 1;
    }
    c.mode = ONLY_NTH;
    c.n = n;
    st = pcall(L, 0, 1);
    outcome_of(L, st, buf, sizeof(buf));
    add_outcome(&o, buf);
    /* the Nth growth never came: the run had fewer than N */
    done = !c.refused;
    lua_close(L);
    if (c.blocks != 0 || c.live != 0) all_freed = 0;
  }
  print_outcomes("one refusal", &o);
  printf("every block freed after lua_close: %s\n", all_freed ? "yes" : "no");
  return 0;
}
