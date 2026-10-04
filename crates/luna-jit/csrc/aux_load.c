/*
 * Loading chunks from files, buffers and strings (luaL_loadfilex,
 * luaL_loadbufferx, luaL_loadstring) through lua_load, as each version's
 * lauxlib does: a first line starting with '#' is skipped (5.2 on also a
 * UTF-8 BOM), a file whose first character is the binary signature is
 * reopened in binary mode, and a file that cannot be opened or read gives
 * LUA_ERRFILE with "cannot open/reopen/read <name>".
 */
#include <errno.h>
#include <string.h>
#include "auxlib.h"

#define SIGNATURE0 '\033'

typedef struct LoadF {
  int n;          /* pre-read characters in buff (5.2 on) */
  int extraline;  /* 5.1: a "\n" to give before the file */
  FILE *f;
  char buff[BUFSIZ > 1024 ? BUFSIZ : 1024];
} LoadF;

static const char *getF(lua_State *L, void *ud, size_t *size) {
  LoadF *lf = (LoadF *)ud;
  (void)L;
  if (VNUM(L) == 501) {
    if (lf->extraline) {
      lf->extraline = 0;
      *size = 1;
      return "\n";
    }
    if (feof(lf->f)) return NULL;
    *size = fread(lf->buff, 1, sizeof(lf->buff), lf->f);
    return (*size > 0) ? lf->buff : NULL;
  }
  if (lf->n > 0) {
    *size = (size_t)lf->n;
    lf->n = 0;
  }
  else {
    if (feof(lf->f)) return NULL;
    *size = fread(lf->buff, 1, sizeof(lf->buff), lf->f);
  }
  return lf->buff;
}

static int errfile(lua_State *L, const char *what, int fnameindex) {
  int err = errno;
  const char *filename = lua_tostring(L, fnameindex) + 1;
  if (VNUM(L) <= 503 || err != 0)
    luna_c_lua_pushfstring(L, "cannot %s %s: %s", what, filename, strerror(err));
  else
    luna_c_lua_pushfstring(L, "cannot %s %s", what, filename);
  lua_remove(L, fnameindex);
  return LUA_ERRERR_OF(L) + 1;
}

/* 5.2 and 5.3 keep the bytes of a partial BOM for the parser; 5.4 on
   drop them and return the first byte */
static int skipBOM(lua_State *L, LoadF *lf) {
  if (VNUM(L) <= 503) {
    const char *p = "\xEF\xBB\xBF";
    int c;
    lf->n = 0;
    do {
      c = getc(lf->f);
      if (c == EOF || c != *(const unsigned char *)p++) return c;
      lf->buff[lf->n++] = (char)c;
    } while (*p != '\0');
    lf->n = 0;
    return getc(lf->f);
  }
  else {
    int c = getc(lf->f);
    if (c == 0xEF && getc(lf->f) == 0xBB && getc(lf->f) == 0xBF) return getc(lf->f);
    return c;
  }
}

static int skipcomment(lua_State *L, LoadF *lf, int *cp) {
  int c = *cp = skipBOM(L, lf);
  if (c == '#') {
    do {
      c = getc(lf->f);
    } while (c != EOF && c != '\n');
    *cp = getc(lf->f);
    return 1;
  }
  return 0;
}

static int load(lua_State *L, lua_Reader r, void *ud, const char *name, const char *mode) {
  if (VNUM(L) == 501) return luna_load_51(L, r, ud, name);
  return lua_load(L, r, ud, name, mode);
}

/* 5.1's reading of the start of a file */
static int start51(lua_State *L, LoadF *lf, const char *filename, int fnameindex) {
  int c = getc(lf->f);
  if (c == '#') {
    lf->extraline = 1;
    while ((c = getc(lf->f)) != EOF && c != '\n') {}
    if (c == '\n') c = getc(lf->f);
  }
  if (c == SIGNATURE0 && filename) {
    lf->f = freopen(filename, "rb", lf->f);
    if (lf->f == NULL) return errfile(L, "reopen", fnameindex);
    while ((c = getc(lf->f)) != EOF && c != SIGNATURE0) {}
    lf->extraline = 0;
  }
  ungetc(c, lf->f);
  return 0;
}

/* 5.2 on */
static int start52(lua_State *L, LoadF *lf, const char *filename, int fnameindex) {
  int c;
  int v = VNUM(L);
  lf->n = 0;
  if (skipcomment(L, lf, &c)) lf->buff[lf->n++] = '\n';
  if (c == SIGNATURE0 && (v >= 504 || filename)) {
    if (v >= 504) lf->n = 0;
    if (filename) {
      if (v >= 504) errno = 0;
      lf->f = freopen(filename, "rb", lf->f);
      if (lf->f == NULL) return errfile(L, "reopen", fnameindex);
      skipcomment(L, lf, &c);
    }
  }
  if (c != EOF) lf->buff[lf->n++] = (char)c;
  return 0;
}

LUNA_HIDDEN int luna_c_luaL_loadfilex(lua_State *L, const char *filename, const char *mode) {
  LoadF lf;
  int status, readstatus;
  int v = VNUM(L);
  int fnameindex = lua_gettop(L) + 1;
  lf.extraline = 0;
  lf.n = 0;
  if (filename == NULL) {
    lua_pushliteral(L, "=stdin");
    lf.f = stdin;
  }
  else {
    luna_c_lua_pushfstring(L, "@%s", filename);
    if (v >= 504) errno = 0;
    lf.f = fopen(filename, "r");
    if (lf.f == NULL) return errfile(L, "open", fnameindex);
  }
  status = v == 501 ? start51(L, &lf, filename, fnameindex)
                    : start52(L, &lf, filename, fnameindex);
  if (status != 0) return status;
  if (v == 504) errno = 0;
  status = load(L, getF, &lf, lua_tostring(L, -1), mode);
  readstatus = ferror(lf.f);
  if (v >= 505) errno = 0;
  if (filename) fclose(lf.f);
  if (readstatus) {
    lua_settop(L, fnameindex);
    return errfile(L, "read", fnameindex);
  }
  lua_remove(L, fnameindex);
  return status;
}

LUNA_HIDDEN int luna_c_luaL_loadfile(lua_State *L, const char *filename) {
  return luna_c_luaL_loadfilex(L, filename, NULL);
}

typedef struct LoadS {
  const char *s;
  size_t size;
} LoadS;

static const char *getS(lua_State *L, void *ud, size_t *size) {
  LoadS *ls = (LoadS *)ud;
  (void)L;
  if (ls->size == 0) return NULL;
  *size = ls->size;
  ls->size = 0;
  return ls->s;
}

LUNA_HIDDEN int luna_c_luaL_loadbufferx(lua_State *L, const char *buff, size_t size,
                                        const char *name, const char *mode) {
  LoadS ls;
  ls.s = buff;
  ls.size = size;
  return load(L, getS, &ls, name, mode);
}

LUNA_HIDDEN int luna_c_luaL_loadbuffer(lua_State *L, const char *buff, size_t size,
                                       const char *name) {
  return luna_c_luaL_loadbufferx(L, buff, size, name, NULL);
}

/* 5.5 loads text only */
LUNA_HIDDEN int luna_c_luaL_loadstring(lua_State *L, const char *s) {
  return luna_c_luaL_loadbufferx(L, s, strlen(s), s, VNUM(L) >= 505 ? "t" : NULL);
}
