/* the first allocation of a big block refused, once: the emergency
   collection that follows keeps what the running function holds (a C
   function up to its top, a Lua function its whole frame) and the
   functions below it, and frees what lies above, among it a dead string a
   returned function left in its registers. What the collector counts
   afterwards, with no collection in between, shows which it kept; it is
   reported from what it counted before, the state's own count being
   luna's. */
#include "alloc_common.h"

static int refuse_big = 0;

/* hands out what is asked, but refuses the first growth to 100000 bytes
   or more once it is armed */
static void *big_once(void *ud, void *ptr, size_t osize, size_t nsize) {
  (void)ud;
  if (nsize == 0) {
    free(ptr);
    return NULL;
  }
  if (refuse_big && nsize >= 100000 && nsize > (ptr != NULL ? osize : 0)) {
    refuse_big = 0;
    return NULL;
  }
  return realloc(ptr, nsize);
}

static const char *scripts[] = {
    /* the big block asked for by a C function (string.rep) */
    "local function deep()\n"
    "  local a1, a2, a3, a4, a5, a6, a7, a8, a9, a10, a11, a12, a13, a14\n"
    "  local s = ('y'):rep(300) .. 'z'\n"
    "  return #s\n"
    "end\n"
    "deep()\n"
    "local before = collectgarbage('count')\n"
    "local big = ('q'):rep(200000)\n"
    "return (collectgarbage('count') - before) * 1024\n",
    /* by a Lua function (a concatenation) */
    "local h = ('q'):rep(60000)\n"
    "local function deep()\n"
    "  local a1, a2, a3, a4, a5, a6, a7, a8, a9, a10, a11, a12, a13, a14\n"
    "  local s = ('y'):rep(300) .. 'z'\n"
    "  return #s\n"
    "end\n"
    "deep()\n"
    "local before = collectgarbage('count')\n"
    "local big = h .. h\n"
    "return (collectgarbage('count') - before) * 1024\n",
    /* with no dead string above the frame */
    "local h = ('q'):rep(60000)\n"
    "local before = collectgarbage('count')\n"
    "local big = h .. h\n"
    "return (collectgarbage('count') - before) * 1024\n",
    /* the second one again, nothing refused */
    "local h = ('q'):rep(60000)\n"
    "local function deep()\n"
    "  local a1, a2, a3, a4, a5, a6, a7, a8, a9, a10, a11, a12, a13, a14\n"
    "  local s = ('y'):rep(300) .. 'z'\n"
    "  return #s\n"
    "end\n"
    "deep()\n"
    "local before = collectgarbage('count')\n"
    "local big = h .. h\n"
    "return (collectgarbage('count') - before) * 1024\n",
    /* a concatenation's operands below a dead string still in the frame:
       5.2 on its top is the operands' end */
    "local h = ('q'):rep(60000)\n"
    "local function f()\n"
    "  local t = {1, 2, 3, 4, 5, 6, ('y'):rep(300) .. 'z'}\n"
    "  t = nil\n"
    "  local before = collectgarbage('count')\n"
    "  local big = h .. h\n"
    "  return (collectgarbage('count') - before) * 1024\n"
    "end\n"
    "return f()\n",
};

int main(void) {
  int i;
  for (i = 0; i < (int)(sizeof(scripts) / sizeof(scripts[0])); i++) {
    lua_State *L = NEWSTATE(big_once, NULL);
    int st;
    luaL_openlibs(L);
    lua_gc(L, LUA_GCCOLLECT, 0);
    /* no automatic step: the collector runs only when the block is refused
       (5.4 and 5.5 collect in an emergency even when stopped, 5.2 and 5.3
       only while running, so there the pause is made long) */
#if LUA_VERSION_NUM >= 504
    lua_gc(L, LUA_GCSTOP, 0);
#else
    lua_gc(L, LUA_GCSETPAUSE, 10000);
    lua_gc(L, LUA_GCCOLLECT, 0);
#endif
    if (luaL_loadstring(L, scripts[i]) != 0) {
      printf("does not load: %s\n", lua_tostring(L, -1));
      return 1;
    }
    /* the last script refuses nothing: the counts without a collection */
    refuse_big = i != 3;
    st = lua_pcall(L, 0, 1, 0);
    if (st == 0)
      printf("%d: %.0f%s\n", i, lua_tonumber(L, -1), refuse_big ? " (nothing refused)" : "");
    else
      printf("%d: error %s\n", i, lua_tostring(L, -1));
    lua_close(L);
  }
  return 0;
}
