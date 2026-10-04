/*
 * The io library of 5.2 to 5.5: file handles, opening and closing,
 * default files, lines, and the module itself (PUC liolib.c).
 */
#include <stdlib.h>
#include "io.h"

#define IO_PREFIX "_IO_"
#define IOPREF_LEN (sizeof(IO_PREFIX) - 1)
#define IO_INPUT (IO_PREFIX "input")
#define IO_OUTPUT (IO_PREFIX "output")

/* 5.4 makes userdata without user values; earlier ones have one */
static void *newud(lua_State *L, size_t sz) {
  return VNUM(L) >= 504 ? lua_newuserdatauv(L, sz, 0) : lua_newuserdata(L, sz);
}

char *luna_io_reserve(lua_State *L, IoBuf *b, size_t extra) {
  if (b->cap - b->n < extra) {
    size_t cap = b->cap ? b->cap : 64;
    char *p;
    while (cap - b->n < extra) {
      if (cap > ((size_t)-1) / 2) {
        free(b->p);
        luna_c_luaL_error(L, "not enough memory");
      }
      cap *= 2;
    }
    p = (char *)realloc(b->p, cap);
    if (p == NULL) {
      free(b->p);
      luna_c_luaL_error(L, "not enough memory");
    }
    b->p = p;
    b->cap = cap;
  }
  return b->p + b->n;
}

void luna_io_push(lua_State *L, IoBuf *b) {
  /* copy into Lua before freeing: the push may raise a memory error */
  char *p = b->p;
  b->p = NULL;
  if (p == NULL) {
    lua_pushlstring(L, "", 0);
    return;
  }
  lua_pushlstring(L, p, b->n);
  free(p);
}

#define tolstream(L) ((luaL_Stream *)luna_c_luaL_checkudata(L, 1, LUA_FILEHANDLE))
#define isclosed(p) ((p)->closef == NULL)

static int io_type(lua_State *L) {
  luaL_Stream *p;
  luna_c_luaL_checkany(L, 1);
  p = (luaL_Stream *)luna_c_luaL_testudata(L, 1, LUA_FILEHANDLE);
  if (p == NULL)
    lua_pushnil(L);
  else if (isclosed(p))
    lua_pushliteral(L, "closed file");
  else
    lua_pushliteral(L, "file");
  return 1;
}

static int f_tostring(lua_State *L) {
  luaL_Stream *p = tolstream(L);
  if (isclosed(p))
    lua_pushliteral(L, "file (closed)");
  else
    luna_c_lua_pushfstring(L, "file (%p)", p->f);
  return 1;
}

static FILE *tofile(lua_State *L) {
  luaL_Stream *p = tolstream(L);
  if (isclosed(p)) luna_c_luaL_error(L, "attempt to use a closed file");
  return p->f;
}

/* a 'closed' handle first, so that an error while opening leaves a
   consistent one */
static luaL_Stream *newprefile(lua_State *L) {
  luaL_Stream *p = (luaL_Stream *)newud(L, sizeof(luaL_Stream));
  p->closef = NULL;
  luna_c_luaL_setmetatable(L, LUA_FILEHANDLE);
  return p;
}

static int aux_close(lua_State *L) {
  luaL_Stream *p = tolstream(L);
  volatile lua_CFunction cf = p->closef;
  p->closef = NULL;
  return (*cf)(L);
}

static int f_close(lua_State *L) {
  tofile(L);
  return aux_close(L);
}

static int io_close(lua_State *L) {
  if (lua_type(L, 1) == LUA_TNONE) lua_getfield(L, REGIDX(L), IO_OUTPUT);
  return f_close(L);
}

static int f_gc(lua_State *L) {
  luaL_Stream *p = tolstream(L);
  if (!isclosed(p) && p->f != NULL) aux_close(L);
  return 0;
}

static int io_fclose(lua_State *L) {
  luaL_Stream *p = tolstream(L);
  if (VNUM(L) >= 504) errno = 0;
  return luna_c_luaL_fileresult(L, fclose(p->f) == 0, NULL);
}

static luaL_Stream *newfile(lua_State *L) {
  luaL_Stream *p = newprefile(L);
  p->f = NULL;
  p->closef = &io_fclose;
  return p;
}

static void opencheck(lua_State *L, const char *fname, const char *mode) {
  luaL_Stream *p = newfile(L);
  p->f = fopen(fname, mode);
  if (p->f == NULL)
    luna_c_luaL_error(L, "cannot open file '%s' (%s)", fname, strerror(errno));
}

/* 5.2: '[rwa]%+?b?'; 5.3 on: '[rwa]%+?' and any number of 'b's */
static int checkmode(lua_State *L, const char *mode) {
  if (*mode == '\0' || strchr("rwa", *(mode++)) == NULL) return 0;
  if (*mode == '+') mode++;
  if (VNUM(L) == 502) {
    if (*mode == 'b') mode++;
    return *mode == '\0';
  }
  return strspn(mode, "b") == strlen(mode);
}

static int io_open(lua_State *L) {
  const char *filename = luna_c_luaL_checklstring(L, 1, NULL);
  const char *mode = luna_c_luaL_optlstring(L, 2, "r", NULL);
  luaL_Stream *p = newfile(L);
  if (!checkmode(L, mode)) luna_c_luaL_argerror(L, 2, "invalid mode");
  if (VNUM(L) >= 504) errno = 0;
  p->f = fopen(filename, mode);
  return (p->f == NULL) ? luna_c_luaL_fileresult(L, 0, filename) : 1;
}

static int io_pclose(lua_State *L) {
  luaL_Stream *p = tolstream(L);
  if (VNUM(L) >= 504) errno = 0;
  return luna_c_luaL_execresult(L, l_pclose(p->f));
}

/* "r" or "w" from 5.3 on (Windows also takes a 'b' or 't' from 5.4) */
static int checkmodep(lua_State *L, const char *m) {
  if (VNUM(L) == 502) return 1;
#if defined(_WIN32)
  if (VNUM(L) >= 504)
    return (m[0] == 'r' || m[0] == 'w') &&
           (m[1] == '\0' || ((m[1] == 'b' || m[1] == 't') && m[2] == '\0'));
#endif
  return (m[0] == 'r' || m[0] == 'w') && m[1] == '\0';
}

static int io_popen(lua_State *L) {
  const char *filename = luna_c_luaL_checklstring(L, 1, NULL);
  const char *mode = luna_c_luaL_optlstring(L, 2, "r", NULL);
  luaL_Stream *p = newprefile(L);
  if (!checkmodep(L, mode)) luna_c_luaL_argerror(L, 2, "invalid mode");
  if (VNUM(L) >= 504) errno = 0;
  p->f = l_popen(filename, mode);
  p->closef = &io_pclose;
  return (p->f == NULL) ? luna_c_luaL_fileresult(L, 0, filename) : 1;
}

static int io_tmpfile(lua_State *L) {
  luaL_Stream *p = newfile(L);
  if (VNUM(L) >= 504) errno = 0;
  p->f = tmpfile();
  return (p->f == NULL) ? luna_c_luaL_fileresult(L, 0, NULL) : 1;
}

static FILE *getiofile(lua_State *L, const char *findex) {
  luaL_Stream *p;
  lua_getfield(L, REGIDX(L), findex);
  p = (luaL_Stream *)lua_touserdata(L, -1);
  if (isclosed(p))
    luna_c_luaL_error(L, VNUM(L) >= 504 ? "default %s file is closed"
                                        : "standard %s file is closed",
                      findex + IOPREF_LEN);
  return p->f;
}

static int g_iofile(lua_State *L, const char *f, const char *mode) {
  if (!lua_isnoneornil(L, 1)) {
    const char *filename = lua_tostring(L, 1);
    if (filename)
      opencheck(L, filename, mode);
    else {
      tofile(L);
      lua_pushvalue(L, 1);
    }
    lua_setfield(L, REGIDX(L), f);
  }
  lua_getfield(L, REGIDX(L), f);
  return 1;
}

static int io_input(lua_State *L) { return g_iofile(L, IO_INPUT, "r"); }

static int io_output(lua_State *L) { return g_iofile(L, IO_OUTPUT, "w"); }

static int io_readline(lua_State *L);

/* the iterator of 'lines': a closure over the file, the number of formats,
   whether to close the file at its end, and the formats */
static void aux_lines(lua_State *L, int toclose) {
  int n = lua_gettop(L) - 1;
  if (VNUM(L) == 502) {
    if (n > LUA_MINSTACK - 3) luna_c_luaL_argerror(L, LUA_MINSTACK - 3, "too many options");
  } else if (n > 250) {
    luna_c_luaL_argerror(L, 252, "too many arguments");
  }
  lua_pushvalue(L, 1);
  lua_pushinteger(L, n);
  lua_pushboolean(L, toclose);
  lua_rotate(L, 2, 3);
  lua_pushcclosure(L, io_readline, 3 + n);
}

static int f_lines(lua_State *L) {
  tofile(L);
  aux_lines(L, 0);
  return 1;
}

/* 5.4 on also return the file to close when the loop ends */
static int io_lines(lua_State *L) {
  int toclose;
  if (lua_type(L, 1) == LUA_TNONE) lua_pushnil(L);
  if (lua_isnil(L, 1)) {
    lua_getfield(L, REGIDX(L), IO_INPUT);
    lua_replace(L, 1);
    tofile(L);
    toclose = 0;
  } else {
    const char *filename = luna_c_luaL_checklstring(L, 1, NULL);
    opencheck(L, filename, "r");
    lua_replace(L, 1);
    toclose = 1;
  }
  aux_lines(L, toclose);
  if (toclose && VNUM(L) >= 504) {
    lua_pushnil(L);
    lua_pushnil(L);
    lua_pushvalue(L, 1);
    return 4;
  }
  return 1;
}

static int io_readline(lua_State *L) {
  luaL_Stream *p = (luaL_Stream *)lua_touserdata(L, UPVAL(L, 1));
  int i, n = (int)lua_tointeger(L, UPVAL(L, 2));
  if (isclosed(p)) return luna_c_luaL_error(L, "file is already closed");
  lua_settop(L, 1);
  if (VNUM(L) >= 503) luna_c_luaL_checkstack(L, n, "too many arguments");
  for (i = 1; i <= n; i++) lua_pushvalue(L, UPVAL(L, 3 + i));
  n = luna_io_g_read(L, p->f, 2);
  if (lua_toboolean(L, -n)) return n;
  if (n > 1) return luna_c_luaL_error(L, "%s", lua_tostring(L, -n + 1));
  if (lua_toboolean(L, UPVAL(L, 3))) {
    lua_settop(L, 0);
    lua_pushvalue(L, UPVAL(L, 1));
    aux_close(L);
  }
  return 0;
}

static int io_read(lua_State *L) { return luna_io_g_read(L, getiofile(L, IO_INPUT), 1); }

static int f_read(lua_State *L) { return luna_io_g_read(L, tofile(L), 2); }

static int io_write(lua_State *L) { return luna_io_g_write(L, getiofile(L, IO_OUTPUT), 1); }

static int f_write(lua_State *L) {
  FILE *f = tofile(L);
  lua_pushvalue(L, 1);
  return luna_io_g_write(L, f, 2);
}

static int io_flush(lua_State *L) { return luna_io_aux_flush(L, getiofile(L, IO_OUTPUT)); }

static int f_flush(lua_State *L) { return luna_io_aux_flush(L, tofile(L)); }

static int f_seek(lua_State *L) {
  tofile(L);
  return luna_io_f_seek(L);
}

static int f_setvbuf(lua_State *L) {
  tofile(L);
  return luna_io_f_setvbuf(L);
}

static const luaL_Reg iolib[] = {
    {"close", io_close}, {"flush", io_flush},   {"input", io_input},
    {"lines", io_lines}, {"open", io_open},     {"output", io_output},
    {"popen", io_popen}, {"read", io_read},     {"tmpfile", io_tmpfile},
    {"type", io_type},   {"write", io_write},   {NULL, NULL}};

/* 5.2/5.3: one table, the metatable, holds methods and metamethods */
static const luaL_Reg flib52[] = {
    {"close", f_close},     {"flush", f_flush},     {"lines", f_lines},
    {"read", f_read},       {"seek", f_seek},       {"setvbuf", f_setvbuf},
    {"write", f_write},     {"__gc", f_gc},         {"__tostring", f_tostring},
    {NULL, NULL}};

static const luaL_Reg meth[] = {
    {"read", f_read},   {"write", f_write}, {"lines", f_lines},     {"flush", f_flush},
    {"seek", f_seek},   {"close", f_close}, {"setvbuf", f_setvbuf}, {NULL, NULL}};

static const luaL_Reg metameth[] = {{"__index", NULL},
                                    {"__gc", f_gc},
                                    {"__close", f_gc},
                                    {"__tostring", f_tostring},
                                    {NULL, NULL}};

static void createmeta(lua_State *L) {
  luna_c_luaL_newmetatable(L, LUA_FILEHANDLE);
  if (VNUM(L) <= 503) {
    lua_pushvalue(L, -1);
    lua_setfield(L, -2, "__index");
    luna_c_luaL_setfuncs(L, flib52, 0);
  } else {
    luna_c_luaL_setfuncs(L, metameth, 0);
    lua_createtable(L, 0, sizeof(meth) / sizeof(meth[0]) - 1);
    luna_c_luaL_setfuncs(L, meth, 0);
    lua_setfield(L, -2, "__index");
  }
  lua_pop(L, 1);
}

static int io_noclose(lua_State *L) {
  luaL_Stream *p = tolstream(L);
  p->closef = &io_noclose;
  lua_pushnil(L);
  lua_pushliteral(L, "cannot close standard file");
  return 2;
}

static void createstdfile(lua_State *L, FILE *f, const char *k, const char *fname) {
  luaL_Stream *p = newprefile(L);
  p->f = f;
  p->closef = &io_noclose;
  if (k != NULL) {
    lua_pushvalue(L, -1);
    lua_setfield(L, REGIDX(L), k);
  }
  lua_setfield(L, -2, fname);
}

int luna_io_open52(lua_State *L) {
  lua_createtable(L, 0, sizeof(iolib) / sizeof(iolib[0]) - 1);
  luna_c_luaL_setfuncs(L, iolib, 0);
  createmeta(L);
  createstdfile(L, stdin, IO_INPUT, "stdin");
  createstdfile(L, stdout, IO_OUTPUT, "stdout");
  createstdfile(L, stderr, NULL, "stderr");
  return 1;
}
