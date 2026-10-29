/* Collections in the middle of a text load: a reader that collects (a full
   collection, then a step) before each piece, a Lua reader function that
   calls collectgarbage, and every allocation of a load refused in turn, so
   that the collection an allocation failure runs falls in it. The chunk's
   functions share names no prototype keeps (labels, from 5.2 on) or keeps
   (locals, fields), which a collection may free between two functions and
   the next one makes again. Freed blocks are filled and never reused, so
   that a string read after it was freed shows. */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "lua.h"
#include "lauxlib.h"
#include "lualib.h"

#if LUA_VERSION_NUM >= 505
#define NEWSTATE(f, ud) lua_newstate(f, ud, 0)
#else
#define NEWSTATE(f, ud) lua_newstate(f, ud)
#endif

#if LUA_VERSION_NUM == 501
#define LOAD(L, r, d, n) lua_load(L, r, d, n)
#else
#define LOAD(L, r, d, n) lua_load(L, r, d, n, NULL)
#endif

/* above the allocations any of the dialects makes for the load */
#define REFUSALS 800

static long allocs = 0, refuse_at = -1;

static void *alloc(void *ud, void *ptr, size_t osize, size_t nsize) {
  void *p;
  (void)ud;
  if (nsize == 0) {
    if (ptr != NULL) memset(ptr, 0xDB, osize);
    return NULL;
  }
  allocs++;
  if (refuse_at >= 0 && allocs == refuse_at) return NULL;
  p = malloc(nsize);
  if (p != NULL && ptr != NULL) {
    memcpy(p, ptr, osize < nsize ? osize : nsize);
    memset(ptr, 0xDB, osize);
  }
  return p;
}

#define FUNCS 24
static char lines[FUNCS + 2][160];
static int nlines;

static void make_source(void) {
  int i;
  nlines = 0;
  sprintf(lines[nlines++], "local out = {}\n");
  for (i = 0; i < FUNCS; i++) {
    if (LUA_VERSION_NUM >= 502)
      sprintf(lines[nlines++],
              "local function f%d() local k = %d goto skip%d k = -1 ::skip%d:: return k end "
              "out[#out+1] = f%d()\n", i, i, i % 3, i % 3, i);
    else
      sprintf(lines[nlines++],
              "local function f%d(alpha) local gamma = { key = alpha } return gamma.key end "
              "out[#out+1] = f%d(%d)\n", i, i, i);
  }
  sprintf(lines[nlines++], "return table.concat(out, ',')\n");
}

typedef struct {
  int next;
  int what;
} Reader;

static const char *reader(lua_State *L, void *ud, size_t *size) {
  Reader *r = (Reader *)ud;
  if (r->what == 1) lua_gc(L, LUA_GCCOLLECT, 0);
  if (r->what == 2) lua_gc(L, LUA_GCSTEP, 0);
  if (r->next >= nlines) {
    *size = 0;
    return NULL;
  }
  *size = strlen(lines[r->next]);
  return lines[r->next++];
}

static lua_State *new_state(void) {
  lua_State *L = NEWSTATE(alloc, NULL);
  luaL_openlibs(L);
  return L;
}

static void from_reader(void) {
  int what, round, k, st;
  for (what = 1; what <= 2; what++) {
    lua_State *L = new_state();
    for (round = 0; round < 5; round++) {
      Reader r;
      r.next = 0;
      r.what = what;
      lua_newtable(L);
      for (k = 0; k < 50; k++) {
        lua_pushfstring(L, "junk%d_%d", round, k);
        lua_rawseti(L, -2, k + 1);
      }
      lua_pop(L, 1);
      st = LOAD(L, reader, &r, "=chunk");
      if (st == 0) st = lua_pcall(L, 0, 1, 0);
      printf("%s %d: %d %s\n", what == 1 ? "collect" : "step", round, st,
             lua_tostring(L, -1) ? lua_tostring(L, -1) : "(nil)");
      lua_pop(L, 1);
    }
    lua_close(L);
  }
}

/* only what is not a memory error, or a chunk that ran wrong, is shown:
   where each refusal falls differs from PUC's allocations */
static void refused_in_turn(void) {
  char src[(FUNCS + 2) * 160];
  long k;
  int i, st, odd = 0;
  src[0] = 0;
  for (i = 0; i < nlines; i++) strcat(src, lines[i]);
  for (k = 1; k <= REFUSALS; k++) {
    lua_State *L = new_state();
    lua_gc(L, LUA_GCCOLLECT, 0);
    refuse_at = allocs + k;
    st = luaL_loadstring(L, src);
    refuse_at = -1;
    if (st == 0) st = lua_pcall(L, 0, 1, 0);
    if ((st != 0 && st != LUA_ERRMEM) ||
        (st == 0 && strncmp(lua_tostring(L, -1), "0,1,2,3", 7) != 0)) {
      printf("refused %ld: %d %s\n", k, st, lua_tostring(L, -1));
      odd++;
    }
    lua_close(L);
  }
  printf("refused in turn: %d odd\n", odd);
}

static void from_lua_reader(void) {
  static const char *drive =
      "local lines = ...\n"
      "for _, how in ipairs{'collect', 'step'} do\n"
      "  for round = 1, 3 do\n"
      "    local i = 0\n"
      "    local f = assert(load(function() collectgarbage(how) i = i + 1 return lines[i] end))\n"
      "    print('lua reader ' .. how, f())\n"
      "  end\n"
      "end\n";
  lua_State *L = new_state();
  int k;
  if (luaL_loadstring(L, drive) != 0) {
    printf("drive: %s\n", lua_tostring(L, -1));
    return;
  }
  lua_newtable(L);
  for (k = 0; k < nlines; k++) {
    lua_pushstring(L, lines[k]);
    lua_rawseti(L, -2, k + 1);
  }
  if (lua_pcall(L, 1, 0, 0) != 0) printf("drive: %s\n", lua_tostring(L, -1));
  fflush(stdout);
  lua_close(L);
}

int main(void) {
  make_source();
  from_reader();
  refused_in_turn();
  from_lua_reader();
  return 0;
}
