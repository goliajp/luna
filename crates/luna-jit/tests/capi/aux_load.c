/* Loading chunks: luaL_loadbuffer(x) with names and modes, luaL_loadstring,
   luaL_loadfile(x) with a first '#' line, a BOM, binary chunks, standard
   input, and files that cannot be opened or read; luaL_dofile and
   luaL_dostring */
#include "aux_common.h"

static const char *const tmpname = "aux_load_tmp.lua";
static const char *const stdinname = "aux_load_stdin.lua";

static void writefile(const char *name, const char *s, size_t len) {
  FILE *f = fopen(name, "wb");
  fwrite(s, 1, len, f);
  fclose(f);
}

/* the status, then the chunk's results or the message */
static void report(lua_State *L, const char *label, int st, int base) {
  printf("%s: status=%d", label, st);
  if (st == 0) {
    int pst = lua_pcall(L, 0, LUA_MULTRET, 0);
    printf(" call=%d", pst);
  }
  show_from(L, "", base + 1);
  lua_settop(L, base);
}

static void loadfile(lua_State *L, const char *label, const char *content, size_t len,
                     const char *mode) {
  int st;
  writefile(tmpname, content, len);
#if LUA_VERSION_NUM >= 502
  st = luaL_loadfilex(L, tmpname, mode);
#else
  (void)mode;
  st = luaL_loadfile(L, tmpname);
#endif
  report(L, label, st, lua_gettop(L) - (st == 0 ? 1 : 1));
}

/* the dump collected outside Lua: 5.5's lua_dump keeps values of its own
   on the stack while it writes */
struct dumpbuf {
  char b[4096];
  size_t n;
};

static int writer(lua_State *L, const void *p, size_t sz, void *ud) {
  struct dumpbuf *d = (struct dumpbuf *)ud;
  (void)L;
  if (d->n + sz > sizeof(d->b)) return 1;
  memcpy(d->b + d->n, p, sz);
  d->n += sz;
  return 0;
}

int main(void) {
  lua_State *L = luaL_newstate();
  int st;
  luaL_openlibs(L);
  lua_pushstring(L, "keep");
  st = luaL_loadbuffer(L, "return 1 + 1", 12, "=buf");
  report(L, "loadbuffer", st, 1);
  st = luaL_loadbuffer(L, "return 1 +", 10, "=buf");
  report(L, "loadbuffer syntax", st, 1);
  st = luaL_loadbuffer(L, "error('e')", 10, "@file.lua");
  report(L, "loadbuffer at name", st, 1);
  st = luaL_loadbuffer(L, "return ...", 10, "plain name");
  report(L, "loadbuffer plain name", st, 1);
  st = luaL_loadbuffer(L, "x = = 1", 7, "a very long chunk name that will not fit in the sixty bytes of a short source");
  report(L, "loadbuffer long name", st, 1);
  st = luaL_loadbuffer(L, "return 'a\0b'", 12, "=nul");
  report(L, "loadbuffer embedded zero", st, 1);
  st = luaL_loadbuffer(L, "", 0, "=empty");
  report(L, "loadbuffer empty", st, 1);
  st = luaL_loadstring(L, "return 'str'");
  report(L, "loadstring", st, 1);
  st = luaL_loadstring(L, "return +");
  report(L, "loadstring syntax", st, 1);
  st = luaL_loadstring(L, "#!shebang\nreturn 1");
  report(L, "loadstring hash", st, 1);
#if LUA_VERSION_NUM >= 502
  st = luaL_loadbufferx(L, "return 2", 8, "=t", "t");
  report(L, "loadbufferx t", st, 1);
  st = luaL_loadbufferx(L, "return 2", 8, "=b", "b");
  report(L, "loadbufferx b", st, 1);
  st = luaL_loadbufferx(L, "return 2", 8, "=bt", "bt");
  report(L, "loadbufferx bt", st, 1);
#endif
  /* a binary chunk, from string.dump */
  {
    static struct dumpbuf d;
    size_t len;
    const char *s;
    luaL_loadstring(L, "return 'binary'");
#if LUA_VERSION_NUM >= 503
    lua_dump(L, writer, &d, 0);
#else
    lua_dump(L, writer, &d);
#endif
    lua_pushlstring(L, d.b, d.n);
    s = lua_tolstring(L, -1, &len);
    st = luaL_loadbuffer(L, s, len, "=bin");
    report(L, "loadbuffer binary", st, 3);
#if LUA_VERSION_NUM >= 502
    st = luaL_loadbufferx(L, s, len, "=bin", "t");
    report(L, "loadbufferx binary as t", st, 3);
#endif
    loadfile(L, "loadfile binary", s, len, NULL);
#if LUA_VERSION_NUM >= 502
    loadfile(L, "loadfilex binary as t", s, len, "t");
#endif
    {
      char tmp[4096];
      memcpy(tmp, "#!/usr/bin/lua\n", 15);
      memcpy(tmp + 15, s, len);
      loadfile(L, "loadfile shebang binary", tmp, len + 15, NULL);
    }
    lua_settop(L, 1);
  }
  loadfile(L, "loadfile", "return 'file', ...", 18, NULL);
  loadfile(L, "loadfile error line", "local x = 1\nerror('at two')", 26, NULL);
  loadfile(L, "loadfile shebang", "#!/usr/bin/lua\nerror('line')", 28, NULL);
  loadfile(L, "loadfile shebang only", "#!/usr/bin/lua", 14, NULL);
  loadfile(L, "loadfile bom", "\xEF\xBB\xBFreturn 'bom'", 16, NULL);
  loadfile(L, "loadfile bom shebang", "\xEF\xBB\xBF#x\nerror('l2')", 18, NULL);
  loadfile(L, "loadfile half bom", "\xEF\xBBreturn 1", 10, NULL);
  loadfile(L, "loadfile empty", "", 0, NULL);
  loadfile(L, "loadfile syntax", "return return", 13, NULL);
#if LUA_VERSION_NUM >= 502
  loadfile(L, "loadfilex b of text", "return 1", 8, "b");
#endif
  remove(tmpname);
  /* a file that is not there */
  st = luaL_loadfile(L, "no_such_file.lua");
  {
    const char *msg = lua_tostring(L, -1);
    printf("loadfile missing: status=%d top=%d prefix=%d\n", st, lua_gettop(L),
           strncmp(msg, "cannot open no_such_file.lua", 28) == 0);
    lua_settop(L, 1);
  }
  /* a directory: POSIX opens it and fails to read it, Windows fails to
     open it (aux_posix checks the reading error) */
  st = luaL_loadfile(L, ".");
  printf("loadfile dir: status=%d top=%d prefix=%d\n", st, lua_gettop(L),
         strncmp(lua_tostring(L, -1), "cannot ", 7) == 0);
  lua_settop(L, 1);
  /* standard input */
  writefile(stdinname, "return 'from stdin'", 19);
  if (freopen(stdinname, "r", stdin) != NULL) {
    st = luaL_loadfile(L, NULL);
    report(L, "loadfile stdin", st, 1);
    luaL_loadfile(L, NULL);
    st = luaL_loadfile(L, NULL);
    report(L, "loadfile stdin again", st, 2);
    lua_settop(L, 1);
  }
  remove(stdinname);
  /* the do macros */
  writefile(tmpname, "return 10, 20", 13);
  st = luaL_dofile(L, tmpname);
  show_from(L, "dofile", 2);
  printf("  status=%d\n", st);
  lua_settop(L, 1);
  remove(tmpname);
  st = luaL_dofile(L, "no_such_file.lua");
  printf("dofile missing: status=%d top=%d\n", st, lua_gettop(L));
  lua_settop(L, 1);
  st = luaL_dostring(L, "return 'done', 2");
  show_from(L, "dostring", 2);
  lua_settop(L, 1);
  st = luaL_dostring(L, "error('dostring error')");
  show_from(L, "dostring error", 2);
  printf("  status=%d\n", st);
  lua_settop(L, 1);
  show_from(L, "end", 1);
  lua_close(L);
  return 0;
}
