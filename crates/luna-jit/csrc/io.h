/*
 * The io library of a C API state, in C over the C library's stdio as
 * PUC's liolib.c is: a file handle is a userdata whose block starts with
 * luaL_Stream (5.1: a FILE *), with the metatable LUA_FILEHANDLE in the
 * registry, so C code reaches it with luaL_checkudata. io_51.c is 5.1's
 * library; io_lib.c and io_rw.c serve 5.2 to 5.5, branching on the
 * version where PUC's differ.
 */
#ifndef LUNA_IO_H
#define LUNA_IO_H

#include <errno.h>
#include <stdio.h>
#include <string.h>
#include "auxlib.h"

#define LUA_FILEHANDLE "FILE*"

typedef struct luaL_Stream {
  FILE *f;
  lua_CFunction closef;
} luaL_Stream;

#if defined(_WIN32)
#define l_popen(c, m) (_popen(c, m))
#define l_pclose(f) (_pclose(f))
#define l_getc(f) getc(f)
#define l_lockfile(f) ((void)0)
#define l_unlockfile(f) ((void)0)
#define l_fseek(f, o, w) _fseeki64(f, o, w)
#define l_ftell(f) _ftelli64(f)
typedef __int64 l_seeknum;
#else
#include <sys/types.h>
#define l_popen(c, m) (fflush(NULL), popen(c, m))
#define l_pclose(f) (pclose(f))
#define l_getc(f) getc_unlocked(f)
#define l_lockfile(f) flockfile(f)
#define l_unlockfile(f) funlockfile(f)
#define l_fseek(f, o, w) fseeko(f, o, w)
#define l_ftell(f) ftello(f)
typedef off_t l_seeknum;
#endif

/* lua_upvalueindex of the state's dialect */
#define UPVAL(L, i) (VNUM(L) == 501 ? -10002 - (i) : REGIDX(L) - (i))

/* 5.1/5.2's LUAL_BUFFERSIZE (BUFSIZ); 5.3 on use 16 * sizeof(void *) *
   sizeof(lua_Integer) */
#define IO_BUFSIZE(L) (VNUM(L) <= 502 ? BUFSIZ : (int)(0x80 * sizeof(void *) * sizeof(lua_Integer)))

/* a growable byte buffer, outside Lua: results are pushed whole */
typedef struct {
  char *p;
  size_t n, cap;
} IoBuf;

char *luna_io_reserve(lua_State *L, IoBuf *b, size_t extra);
void luna_io_push(lua_State *L, IoBuf *b);

/* the other functions of the library these files share */
int luna_c_luaL_fileresult(lua_State *L, int stat, const char *fname);
int luna_c_luaL_execresult(lua_State *L, int stat);
void *luna_c_luaL_checkudata(lua_State *L, int ud, const char *tname);
void *luna_c_luaL_testudata(lua_State *L, int ud, const char *tname);
void luna_c_luaL_setmetatable(lua_State *L, const char *tname);
int luna_c_luaL_checkoption(lua_State *L, int arg, const char *def,
                            const char *const lst[]);
lua_Integer luna_c_luaL_checkinteger(lua_State *L, int arg);
lua_Integer luna_c_luaL_optinteger(lua_State *L, int arg, lua_Integer def);
lua_Number luna_c_luaL_optnumber(lua_State *L, int arg, lua_Number def);
void luna_c_luaL_checkany(lua_State *L, int arg);
void luna_c_luaL_register(lua_State *L, const char *libname, const luaL_Reg *l);
void *lua_newuserdata(lua_State *L, size_t sz);
lua_CFunction lua_tocfunction(lua_State *L, int idx);
size_t lua_stringtonumber(lua_State *L, const char *s);
void lua_getfenv(lua_State *L, int idx);
int lua_setfenv(lua_State *L, int idx);

int luna_io_open52(lua_State *L);
int luna_io_open51(lua_State *L);

/* io_rw.c: reading, writing and the other methods of 5.2 on */
int luna_io_g_read(lua_State *L, FILE *f, int first);
int luna_io_g_write(lua_State *L, FILE *f, int arg);
int luna_io_f_seek(lua_State *L);
int luna_io_f_setvbuf(lua_State *L);
int luna_io_aux_flush(lua_State *L, FILE *f);

#endif
