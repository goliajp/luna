/* an allocation refused while a finalizer runs: the garbage the finalizer
   made itself (no collection step runs during a finalizer) can be freed by
   an emergency collection, after which the allocation is retried */
#include "alloc_common.h"

static const char *script =
    "local res = 'not run'\n"
    "local function body()\n"
    "  local t = {}\n"
    "  for i = 1, 400 do t[i] = string.rep('y', 4000 + i) end\n"
    "  t = nil\n"
    "  return #string.rep('z', 600000)\n"
    "end\n"
    "local function fin()\n"
    "  local ok, r = pcall(body)\n"
    "  res = tostring(ok) .. ' ' .. tostring(r)\n"
    "end\n"
    "if newproxy then\n"
    "  local p = newproxy(true); getmetatable(p).__gc = fin; p = nil\n"
    "else\n"
    "  setmetatable({}, {__gc = fin})\n"
    "end\n"
    "collectgarbage()\n"
    "return res\n";

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
  /* room for about 2 MB more: the finalizer's garbage (some 1.6 MB) and the
     big string (the buffer and the result, some 1.2 MB) do not fit together */
  c.mode = LIMIT;
  c.limit = c.live + (2 << 20);
  st = pcall(L, 0, 1);
  outcome_of(L, st, buf, sizeof(buf));
  printf("allocation refused in a finalizer: %s\n", buf);
  lua_close(L);
  printf("every block freed after lua_close: %s\n", c.blocks == 0 && c.live == 0 ? "yes" : "no");
  return 0;
}
