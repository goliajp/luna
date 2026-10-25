/* the growth refused, once, is one a new closure asks for: its own block,
   or the block of its second upvalue, a variable no open upvalue holds
   yet. From 5.2 on the emergency collection that follows keeps the first
   upvalue, which the closure (made first, holding each upvalue as it is
   found) already holds, and frees the dead open upvalue of a variable the
   closure does not capture. What the collector counts across the closure
   shows what it kept and freed. */
#include "alloc_common.h"

static Ctl c;

/* arm(k): refuse the growth after the next k ones */
static int arm(lua_State *L) {
  c.mode = ONLY_NTH;
  c.n = (long)luaL_checkinteger(L, 1);
  c.refused = 0;
  return 0;
}

static const char *script =
    "local arm = ...\n"
    "local function f(k)\n"
    "  local d = {}\n"
    "  do local g = function() return d end end\n"
    "  local x, y = {}, {}\n"
    "  local keep = function() return x end\n"
    "  local before = collectgarbage('count')\n"
    "  if k then arm(k) end\n"
    "  local h = function() return x, y end\n"
    "  local after = collectgarbage('count')\n"
    "  local a, b = h()\n"
    "  return (after - before) * 1024, a == x and b == y, keep() == x\n"
    "end\n"
    "local out = {}\n"
    "for _, k in ipairs({false, 0, 1}) do\n"
    "  local n, same, kept = f(k)\n"
    "  out[#out + 1] = tostring(k) .. ': ' .. n .. ' ' .. tostring(same) .. ' ' .. tostring(kept)\n"
    "end\n"
    "return table.concat(out, '\\n')\n";

int main(void) {
  lua_State *L;
  int st;
  memset(&c, 0, sizeof(c));
  L = NEWSTATE(ctl_alloc, &c);
  luaL_openlibs(L);
  lua_gc(L, LUA_GCCOLLECT, 0);
  /* no automatic step: the collector runs only when the growth is refused
     (5.4 and 5.5 collect in an emergency even when stopped, 5.2 and 5.3
     only while running, so there the pause is made long) */
#if LUA_VERSION_NUM >= 504
  lua_gc(L, LUA_GCSTOP, 0);
#else
  lua_gc(L, LUA_GCSETPAUSE, 10000);
  lua_gc(L, LUA_GCCOLLECT, 0);
#endif
  if (luaL_loadstring(L, script) != 0) {
    printf("does not load: %s\n", lua_tostring(L, -1));
    return 1;
  }
  lua_pushcfunction(L, arm);
  st = lua_pcall(L, 1, 1, 0);
  printf("%s\n%s\n", status_name(st), lua_tostring(L, -1));
  printf("refused: %s\n", c.refused ? "yes" : "no");
  lua_close(L);
  printf("every block freed after lua_close: %s\n", c.blocks == 0 && c.live == 0 ? "yes" : "no");
  return 0;
}
