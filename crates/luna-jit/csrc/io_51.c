/*
 * The io library of 5.1 (PUC 5.1's liolib.c): a file handle is a userdata
 * holding a FILE *, NULL once closed; how it closes is the "__close" field
 * of its environment, and the default files are fields 1 and 2 of the
 * library functions' environment.
 */
#include <stdlib.h>
#include "io.h"

#define IO_INPUT 1
#define IO_OUTPUT 2
#define ENVIRONINDEX (-10001)
#define REGISTRY (-10000)

static const char *const fnames[] = {"input", "output"};

static int pushresult(lua_State *L, int i, const char *filename) {
  int en = errno;
  if (i) {
    lua_pushboolean(L, 1);
    return 1;
  }
  lua_pushnil(L);
  if (filename)
    luna_c_lua_pushfstring(L, "%s: %s", filename, strerror(en));
  else
    luna_c_lua_pushfstring(L, "%s", strerror(en));
  lua_pushinteger(L, en);
  return 3;
}

static void fileerror(lua_State *L, int arg, const char *filename) {
  luna_c_lua_pushfstring(L, "%s: %s", filename, strerror(errno));
  luna_c_luaL_argerror(L, arg, lua_tostring(L, -1));
}

#define tofilep(L) ((FILE **)luna_c_luaL_checkudata(L, 1, LUA_FILEHANDLE))

static int io_type(lua_State *L) {
  void *ud;
  luna_c_luaL_checkany(L, 1);
  ud = lua_touserdata(L, 1);
  lua_getfield(L, REGISTRY, LUA_FILEHANDLE);
  if (ud == NULL || !lua_getmetatable(L, 1) || !lua_rawequal(L, -2, -1))
    lua_pushnil(L);
  else if (*((FILE **)ud) == NULL)
    lua_pushliteral(L, "closed file");
  else
    lua_pushliteral(L, "file");
  return 1;
}

static FILE *tofile(lua_State *L) {
  FILE **f = tofilep(L);
  if (*f == NULL) luna_c_luaL_error(L, "attempt to use a closed file");
  return *f;
}

/* a 'closed' handle first, so that an error while opening leaves a
   consistent one; it takes the running function's environment */
static FILE **newfile(lua_State *L) {
  FILE **pf = (FILE **)lua_newuserdata(L, sizeof(FILE *));
  *pf = NULL;
  lua_getfield(L, REGISTRY, LUA_FILEHANDLE);
  lua_setmetatable(L, -2);
  return pf;
}

static int io_noclose(lua_State *L) {
  lua_pushnil(L);
  lua_pushliteral(L, "cannot close standard file");
  return 2;
}

static int io_pclose(lua_State *L) {
  FILE **p = tofilep(L);
  int ok = l_pclose(*p) != -1;
  *p = NULL;
  return pushresult(L, ok, NULL);
}

static int io_fclose(lua_State *L) {
  FILE **p = tofilep(L);
  int ok = (fclose(*p) == 0);
  *p = NULL;
  return pushresult(L, ok, NULL);
}

static int aux_close(lua_State *L) {
  lua_getfenv(L, 1);
  lua_getfield(L, -1, "__close");
  return (lua_tocfunction(L, -1))(L);
}

static int io_close(lua_State *L) {
  if (lua_type(L, 1) == LUA_TNONE) lua_rawgeti(L, ENVIRONINDEX, IO_OUTPUT);
  tofile(L);
  return aux_close(L);
}

static int io_gc(lua_State *L) {
  FILE *f = *tofilep(L);
  if (f != NULL) aux_close(L);
  return 0;
}

static int io_tostring(lua_State *L) {
  FILE *f = *tofilep(L);
  if (f == NULL)
    lua_pushliteral(L, "file (closed)");
  else
    luna_c_lua_pushfstring(L, "file (%p)", f);
  return 1;
}

static int io_open(lua_State *L) {
  const char *filename = luna_c_luaL_checklstring(L, 1, NULL);
  const char *mode = luna_c_luaL_optlstring(L, 2, "r", NULL);
  FILE **pf = newfile(L);
  *pf = fopen(filename, mode);
  return (*pf == NULL) ? pushresult(L, 0, filename) : 1;
}

/* its environment's "__close" is io_pclose */
static int io_popen(lua_State *L) {
  const char *filename = luna_c_luaL_checklstring(L, 1, NULL);
  const char *mode = luna_c_luaL_optlstring(L, 2, "r", NULL);
  FILE **pf = newfile(L);
  *pf = l_popen(filename, mode);
  return (*pf == NULL) ? pushresult(L, 0, filename) : 1;
}

static int io_tmpfile(lua_State *L) {
  FILE **pf = newfile(L);
  *pf = tmpfile();
  return (*pf == NULL) ? pushresult(L, 0, NULL) : 1;
}

static FILE *getiofile(lua_State *L, int findex) {
  FILE *f;
  lua_rawgeti(L, ENVIRONINDEX, findex);
  f = *(FILE **)lua_touserdata(L, -1);
  if (f == NULL) luna_c_luaL_error(L, "standard %s file is closed", fnames[findex - 1]);
  return f;
}

static int g_iofile(lua_State *L, int f, const char *mode) {
  if (!lua_isnoneornil(L, 1)) {
    const char *filename = lua_tostring(L, 1);
    if (filename) {
      FILE **pf = newfile(L);
      *pf = fopen(filename, mode);
      if (*pf == NULL) fileerror(L, 1, filename);
    } else {
      tofile(L);
      lua_pushvalue(L, 1);
    }
    lua_rawseti(L, ENVIRONINDEX, f);
  }
  lua_rawgeti(L, ENVIRONINDEX, f);
  return 1;
}

static int io_input(lua_State *L) { return g_iofile(L, IO_INPUT, "r"); }

static int io_output(lua_State *L) { return g_iofile(L, IO_OUTPUT, "w"); }

static int io_readline(lua_State *L);

static void aux_lines(lua_State *L, int idx, int toclose) {
  lua_pushvalue(L, idx);
  lua_pushboolean(L, toclose);
  lua_pushcclosure(L, io_readline, 2);
}

static int f_lines(lua_State *L) {
  tofile(L);
  aux_lines(L, 1, 0);
  return 1;
}

static int io_lines(lua_State *L) {
  if (lua_isnoneornil(L, 1)) {
    lua_rawgeti(L, ENVIRONINDEX, IO_INPUT);
    return f_lines(L);
  } else {
    const char *filename = luna_c_luaL_checklstring(L, 1, NULL);
    FILE **pf = newfile(L);
    *pf = fopen(filename, "r");
    if (*pf == NULL) fileerror(L, 1, filename);
    aux_lines(L, lua_gettop(L), 1);
    return 1;
  }
}

static int read_number(lua_State *L, FILE *f) {
  double d;
  if (fscanf(f, "%lf", &d) == 1) {
    lua_pushnumber(L, d);
    return 1;
  }
  lua_pushnil(L);
  return 0;
}

static int test_eof(lua_State *L, FILE *f) {
  int c = getc(f);
  ungetc(c, f);
  lua_pushlstring(L, "", 0);
  return c != EOF;
}

static int read_line(lua_State *L, FILE *f) {
  IoBuf b = {NULL, 0, 0};
  for (;;) {
    char *p = luna_io_reserve(L, &b, BUFSIZ);
    size_t l;
    if (fgets(p, BUFSIZ, f) == NULL) {
      luna_io_push(L, &b);
      return lua_objlen(L, -1) > 0;
    }
    l = strlen(p);
    if (l == 0 || p[l - 1] != '\n') {
      b.n += l;
    } else {
      b.n += l - 1;
      luna_io_push(L, &b);
      return 1;
    }
  }
}

static int read_chars(lua_State *L, FILE *f, size_t n) {
  IoBuf b = {NULL, 0, 0};
  size_t rlen = BUFSIZ, nr;
  do {
    char *p = luna_io_reserve(L, &b, BUFSIZ);
    if (rlen > n) rlen = n;
    nr = fread(p, 1, rlen, f);
    b.n += nr;
    n -= nr;
  } while (n > 0 && nr == rlen);
  luna_io_push(L, &b);
  return n == 0 || lua_objlen(L, -1) > 0;
}

static int g_read(lua_State *L, FILE *f, int first) {
  int nargs = lua_gettop(L) - 1;
  int success, n;
  clearerr(f);
  if (nargs == 0) {
    success = read_line(L, f);
    n = first + 1;
  } else {
    luna_c_luaL_checkstack(L, nargs + LUA_MINSTACK, "too many arguments");
    success = 1;
    for (n = first; nargs-- && success; n++) {
      if (lua_type(L, n) == LUA_TNUMBER) {
        size_t l = (size_t)lua_tointeger(L, n);
        success = (l == 0) ? test_eof(L, f) : read_chars(L, f, l);
      } else {
        const char *p = lua_tostring(L, n);
        if (!(p && p[0] == '*')) luna_c_luaL_argerror(L, n, "invalid option");
        switch (p[1]) {
          case 'n':
            success = read_number(L, f);
            break;
          case 'l':
            success = read_line(L, f);
            break;
          case 'a':
            read_chars(L, f, ~((size_t)0));
            success = 1;
            break;
          default:
            return luna_c_luaL_argerror(L, n, "invalid format");
        }
      }
    }
  }
  if (ferror(f)) return pushresult(L, 0, NULL);
  if (!success) {
    lua_pop(L, 1);
    lua_pushnil(L);
  }
  return n - first;
}

static int io_read(lua_State *L) { return g_read(L, getiofile(L, IO_INPUT), 1); }

static int f_read(lua_State *L) { return g_read(L, tofile(L), 2); }

static int io_readline(lua_State *L) {
  FILE *f = *(FILE **)lua_touserdata(L, UPVAL(L, 1));
  int sucess;
  if (f == NULL) luna_c_luaL_error(L, "file is already closed");
  sucess = read_line(L, f);
  if (ferror(f)) return luna_c_luaL_error(L, "%s", strerror(errno));
  if (sucess) return 1;
  if (lua_toboolean(L, UPVAL(L, 2))) {
    lua_settop(L, 0);
    lua_pushvalue(L, UPVAL(L, 1));
    aux_close(L);
  }
  return 0;
}

/* returns true, not the file */
static int g_write(lua_State *L, FILE *f, int arg) {
  int nargs = lua_gettop(L) - 1;
  int status = 1;
  for (; nargs--; arg++) {
    if (lua_type(L, arg) == LUA_TNUMBER) {
      status = status && fprintf(f, "%.14g", (double)lua_tonumber(L, arg)) > 0;
    } else {
      size_t l;
      const char *s = luna_c_luaL_checklstring(L, arg, &l);
      status = status && (fwrite(s, 1, l, f) == l);
    }
  }
  return pushresult(L, status, NULL);
}

static int io_write(lua_State *L) { return g_write(L, getiofile(L, IO_OUTPUT), 1); }

static int f_write(lua_State *L) { return g_write(L, tofile(L), 2); }

static int f_seek(lua_State *L) {
  static const int mode[] = {SEEK_SET, SEEK_CUR, SEEK_END};
  static const char *const modenames[] = {"set", "cur", "end", NULL};
  FILE *f = tofile(L);
  int op = luna_c_luaL_checkoption(L, 2, "cur", modenames);
  long offset = (long)luna_c_luaL_optinteger(L, 3, 0);
  op = fseek(f, offset, mode[op]);
  if (op) return pushresult(L, 0, NULL);
  lua_pushinteger(L, ftell(f));
  return 1;
}

static int f_setvbuf(lua_State *L) {
  static const int mode[] = {_IONBF, _IOFBF, _IOLBF};
  static const char *const modenames[] = {"no", "full", "line", NULL};
  FILE *f = tofile(L);
  int op = luna_c_luaL_checkoption(L, 2, NULL, modenames);
  lua_Integer sz = luna_c_luaL_optinteger(L, 3, BUFSIZ);
  int res = setvbuf(f, NULL, mode[op], (size_t)sz);
  return pushresult(L, res == 0, NULL);
}

static int io_flush(lua_State *L) { return pushresult(L, fflush(getiofile(L, IO_OUTPUT)) == 0, NULL); }

static int f_flush(lua_State *L) { return pushresult(L, fflush(tofile(L)) == 0, NULL); }

static const luaL_Reg iolib[] = {
    {"close", io_close}, {"flush", io_flush}, {"input", io_input},
    {"lines", io_lines}, {"open", io_open},   {"output", io_output},
    {"popen", io_popen}, {"read", io_read},   {"tmpfile", io_tmpfile},
    {"type", io_type},   {"write", io_write}, {NULL, NULL}};

static const luaL_Reg flib[] = {
    {"close", io_close}, {"flush", f_flush},       {"lines", f_lines},
    {"read", f_read},    {"seek", f_seek},         {"setvbuf", f_setvbuf},
    {"write", f_write},  {"__gc", io_gc},          {"__tostring", io_tostring},
    {NULL, NULL}};

static void createmeta(lua_State *L) {
  luna_c_luaL_newmetatable(L, LUA_FILEHANDLE);
  lua_pushvalue(L, -1);
  lua_setfield(L, -2, "__index");
  luna_c_luaL_register(L, NULL, flib);
}

static void createstdfile(lua_State *L, FILE *f, int k, const char *fname) {
  *newfile(L) = f;
  if (k > 0) {
    lua_pushvalue(L, -1);
    lua_rawseti(L, ENVIRONINDEX, k);
  }
  lua_pushvalue(L, -2);
  lua_setfenv(L, -2);
  lua_setfield(L, -3, fname);
}

static void newfenv(lua_State *L, lua_CFunction cls) {
  lua_createtable(L, 0, 1);
  lua_pushcfunction(L, cls);
  lua_setfield(L, -2, "__close");
}

int luna_io_open51(lua_State *L) {
  createmeta(L);
  newfenv(L, io_fclose);
  lua_replace(L, ENVIRONINDEX);
  luna_c_luaL_register(L, "io", iolib);
  newfenv(L, io_noclose);
  createstdfile(L, stdin, IO_INPUT, "stdin");
  createstdfile(L, stdout, IO_OUTPUT, "stdout");
  createstdfile(L, stderr, 0, "stderr");
  lua_pop(L, 1);
  lua_getfield(L, -1, "popen");
  newfenv(L, io_pclose);
  lua_setfenv(L, -2);
  lua_pop(L, 1);
  return 1;
}
