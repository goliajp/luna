/*
 * Reading and writing file handles in 5.2 to 5.5 (PUC liolib.c's READ
 * section, g_write, seek, setvbuf and flush).
 */
#include <ctype.h>
#include <locale.h>
#include <stdlib.h>
#include "io.h"

#define L_MAXLENNUM 200

/* 5.3 on: read a prefix of a numeral and let lua_stringtonumber judge it */
typedef struct {
  FILE *f;
  int c;
  int n;
  char buff[L_MAXLENNUM + 1];
} RN;

static int nextc(RN *rn) {
  if (rn->n >= L_MAXLENNUM) {
    rn->buff[0] = '\0';
    return 0;
  }
  rn->buff[rn->n++] = (char)rn->c;
  rn->c = l_getc(rn->f);
  return 1;
}

static int test2(RN *rn, const char *set) {
  if (rn->c == set[0] || rn->c == set[1]) return nextc(rn);
  return 0;
}

static int readdigits(RN *rn, int hex) {
  int count = 0;
  while ((hex ? isxdigit(rn->c) : isdigit(rn->c)) && nextc(rn)) count++;
  return count;
}

static int read_number(lua_State *L, FILE *f) {
  RN rn;
  int count = 0, hex = 0;
  char decp[2];
  if (VNUM(L) == 502) {
    double d;
    if (fscanf(f, "%lf", &d) == 1) {
      lua_pushnumber(L, d);
      return 1;
    }
    lua_pushnil(L);
    return 0;
  }
  rn.f = f;
  rn.n = 0;
  decp[0] = localeconv()->decimal_point[0];
  decp[1] = '.';
  l_lockfile(rn.f);
  do {
    rn.c = l_getc(rn.f);
  } while (isspace(rn.c));
  test2(&rn, "-+");
  if (test2(&rn, "00")) {
    if (test2(&rn, "xX"))
      hex = 1;
    else
      count = 1;
  }
  count += readdigits(&rn, hex);
  if (test2(&rn, decp)) count += readdigits(&rn, hex);
  if (count > 0 && test2(&rn, (hex ? "pP" : "eE"))) {
    test2(&rn, "-+");
    readdigits(&rn, 0);
  }
  ungetc(rn.c, rn.f);
  l_unlockfile(rn.f);
  rn.buff[rn.n] = '\0';
  if (lua_stringtonumber(L, rn.buff)) return 1;
  lua_pushnil(L);
  return 0;
}

static int test_eof(lua_State *L, FILE *f) {
  int c = getc(f);
  ungetc(c, f);
  lua_pushlstring(L, "", 0);
  return c != EOF;
}

/* 5.2 reads a line with fgets (so a NUL ends a piece early), later
   versions a character at a time */
static int read_line(lua_State *L, FILE *f, int chop) {
  IoBuf b = {NULL, 0, 0};
  size_t chunk = (size_t)IO_BUFSIZE(L);
  int c = '\0';
  if (VNUM(L) == 502) {
    for (;;) {
      char *p = luna_io_reserve(L, &b, chunk);
      size_t l;
      if (fgets(p, (int)chunk, f) == NULL) {
        luna_io_push(L, &b);
        return lua_rawlen(L, -1) > 0;
      }
      l = strlen(p);
      if (l == 0 || p[l - 1] != '\n') {
        b.n += l;
      } else {
        b.n += l - (size_t)chop;
        luna_io_push(L, &b);
        return 1;
      }
    }
  }
  do {
    char *buff = luna_io_reserve(L, &b, chunk);
    size_t i = 0;
    l_lockfile(f);
    while (i < chunk && (c = l_getc(f)) != EOF && c != '\n') buff[i++] = (char)c;
    l_unlockfile(f);
    b.n += i;
  } while (c != EOF && c != '\n');
  if (!chop && c == '\n') *luna_io_reserve(L, &b, 1) = '\n', b.n++;
  luna_io_push(L, &b);
  return c == '\n' || lua_rawlen(L, -1) > 0;
}

/* 5.2 doubles each read; later versions read LUAL_BUFFERSIZE at a time */
static void read_all(lua_State *L, FILE *f) {
  IoBuf b = {NULL, 0, 0};
  size_t rlen = (size_t)IO_BUFSIZE(L), nr;
  for (;;) {
    char *p = luna_io_reserve(L, &b, rlen);
    nr = fread(p, 1, rlen, f);
    b.n += nr;
    if (nr < rlen) break;
    if (VNUM(L) == 502 && rlen <= ((size_t)-1) / 4) rlen *= 2;
  }
  luna_io_push(L, &b);
}

static int read_chars(lua_State *L, FILE *f, size_t n) {
  IoBuf b = {NULL, 0, 0};
  char *p = luna_io_reserve(L, &b, n);
  size_t nr = fread(p, 1, n, f);
  b.n = nr;
  luna_io_push(L, &b);
  return nr > 0;
}

int luna_io_g_read(lua_State *L, FILE *f, int first) {
  int nargs = lua_gettop(L) - 1;
  int n, success;
  clearerr(f);
  if (VNUM(L) >= 504) errno = 0;
  if (nargs == 0) {
    success = read_line(L, f, 1);
    n = first + 1;
  } else {
    luna_c_luaL_checkstack(L, nargs + LUA_MINSTACK, "too many arguments");
    success = 1;
    for (n = first; nargs-- && success; n++) {
      if (lua_type(L, n) == LUA_TNUMBER) {
        size_t l = (size_t)(VNUM(L) == 502 ? lua_tointeger(L, n) : luna_c_luaL_checkinteger(L, n));
        success = (l == 0) ? test_eof(L, f) : read_chars(L, f, l);
      } else {
        const char *p;
        if (VNUM(L) == 502) {
          p = lua_tostring(L, n);
          if (!(p && p[0] == '*')) luna_c_luaL_argerror(L, n, "invalid option");
          p++;
        } else {
          p = luna_c_luaL_checklstring(L, n, NULL);
          if (*p == '*') p++;
        }
        switch (*p) {
          case 'n':
            success = read_number(L, f);
            break;
          case 'l':
            success = read_line(L, f, 1);
            break;
          case 'L':
            success = read_line(L, f, 0);
            break;
          case 'a':
            read_all(L, f);
            success = 1;
            break;
          default:
            return luna_c_luaL_argerror(L, n, "invalid format");
        }
      }
    }
  }
  if (ferror(f)) return luna_c_luaL_fileresult(L, 0, NULL);
  if (!success) {
    lua_pop(L, 1);
    lua_pushnil(L);
  }
  return n - first;
}

/* a number argument as each version writes it: 5.2 with "%.14g", 5.3
   and 5.4 integers with "%lld", 5.5 as tostring does */
static int write_number(lua_State *L, FILE *f, int arg, size_t *len) {
  char buff[64];
  if (VNUM(L) >= 505) {
    unsigned l = lua_numbertocstring(L, arg, buff);
    *len = l - 1;
    return (int)fwrite(buff, 1, *len, f);
  }
  if (VNUM(L) >= 503 && lua_isinteger(L, arg))
    return fprintf(f, "%lld", (long long)lua_tointeger(L, arg));
  return fprintf(f, "%.14g", (double)lua_tonumber(L, arg));
}

int luna_io_g_write(lua_State *L, FILE *f, int arg) {
  int nargs = lua_gettop(L) - arg;
  int status = 1;
  size_t total = 0;
  if (VNUM(L) >= 504) errno = 0;
  for (; nargs--; arg++) {
    size_t len = 0, wrote;
    if (lua_type(L, arg) == LUA_TNUMBER) {
      int r = write_number(L, f, arg, &len);
      if (VNUM(L) < 505) {
        status = status && r > 0;
        continue;
      }
      wrote = (size_t)r;
    } else {
      const char *s = luna_c_luaL_checklstring(L, arg, &len);
      wrote = fwrite(s, 1, len, f);
      if (VNUM(L) < 505) {
        status = status && wrote == len;
        continue;
      }
    }
    total += wrote;
    if (wrote < len) {
      int n = luna_c_luaL_fileresult(L, 0, NULL);
      lua_pushinteger(L, (lua_Integer)total);
      return n + 1;
    }
  }
  if (status) return 1;
  return luna_c_luaL_fileresult(L, status, NULL);
}

int luna_io_f_seek(lua_State *L) {
  static const int mode[] = {SEEK_SET, SEEK_CUR, SEEK_END};
  static const char *const modenames[] = {"set", "cur", "end", NULL};
  FILE *f = ((luaL_Stream *)lua_touserdata(L, 1))->f;
  int op = luna_c_luaL_checkoption(L, 2, "cur", modenames);
  l_seeknum offset;
  if (VNUM(L) == 502) {
    lua_Number p3 = luna_c_luaL_optnumber(L, 3, 0);
    offset = (l_seeknum)p3;
    if ((lua_Number)offset != p3) luna_c_luaL_argerror(L, 3, "not an integer in proper range");
  } else {
    lua_Integer p3 = luna_c_luaL_optinteger(L, 3, 0);
    offset = (l_seeknum)p3;
    if ((lua_Integer)offset != p3) luna_c_luaL_argerror(L, 3, "not an integer in proper range");
  }
  if (VNUM(L) >= 504) errno = 0;
  op = l_fseek(f, offset, mode[op]);
  if (op) return luna_c_luaL_fileresult(L, 0, NULL);
  if (VNUM(L) == 502)
    lua_pushnumber(L, (lua_Number)l_ftell(f));
  else
    lua_pushinteger(L, (lua_Integer)l_ftell(f));
  return 1;
}

int luna_io_f_setvbuf(lua_State *L) {
  static const int mode[] = {_IONBF, _IOFBF, _IOLBF};
  static const char *const modenames[] = {"no", "full", "line", NULL};
  FILE *f = ((luaL_Stream *)lua_touserdata(L, 1))->f;
  int op = luna_c_luaL_checkoption(L, 2, NULL, modenames);
  lua_Integer sz = luna_c_luaL_optinteger(L, 3, IO_BUFSIZE(L));
  int res;
  if (VNUM(L) >= 504) errno = 0;
  res = setvbuf(f, NULL, mode[op], (size_t)sz);
  return luna_c_luaL_fileresult(L, res == 0, NULL);
}

int luna_io_aux_flush(lua_State *L, FILE *f) {
  if (VNUM(L) >= 504) errno = 0;
  return luna_c_luaL_fileresult(L, fflush(f) == 0, NULL);
}
