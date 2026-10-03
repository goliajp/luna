/*
** luna's luaconf.h for the Lua 5.2 dialect: the configuration of a
** default 64-bit build, as PUC-Rio's luaconf.h gives it.
** See Copyright Notice in lua.h
*/


#ifndef lconfig_h
#define lconfig_h

#include <limits.h>
#include <stddef.h>


/* no install prefix: only the current-directory templates */
#define LUA_PATH_DEFAULT	"./?.lua;./?/init.lua"
#define LUA_CPATH_DEFAULT	""

#if defined(_WIN32)
#define LUA_DIRSEP	"\\"
#else
#define LUA_DIRSEP	"/"
#endif


/* name of the environment variable of a function */
#define LUA_ENV		"_ENV"


/*
** Marks for exported symbols
*/
#if defined(LUA_BUILD_AS_DLL)	/* { */

#if defined(LUA_CORE) || defined(LUA_LIB)	/* { */
#define LUA_API __declspec(dllexport)
#else						/* }{ */
#define LUA_API __declspec(dllimport)
#endif						/* } */

#else				/* }{ */

#define LUA_API		extern

#endif				/* } */

#define LUALIB_API	LUA_API
#define LUAMOD_API	LUALIB_API


/* quote names in messages */
#define LUA_QL(x)	"'" x "'"
#define LUA_QS		LUA_QL("%s")


/* maximum size for the description of the source of a function */
#define LUA_IDSIZE	60


/* print an error message (it needs <stdio.h>) */
#define luai_writestringerror(s,p) \
	(fprintf(stderr, (s), (p)), fflush(stderr))


/*
** {==================================================================
** Compatibility with previous versions
** ===================================================================
*/

#if defined(LUA_COMPAT_ALL)	/* { */

#define LUA_COMPAT_UNPACK
#define LUA_COMPAT_LOADERS

#define lua_cpcall(L,f,u)  \
	(lua_pushcfunction(L, (f)), \
	 lua_pushlightuserdata(L,(u)), \
	 lua_pcall(L,1,0,0))

#define LUA_COMPAT_LOG10
#define LUA_COMPAT_LOADSTRING
#define LUA_COMPAT_MAXN

#define lua_strlen(L,i)		lua_rawlen(L, (i))

#define lua_objlen(L,i)		lua_rawlen(L, (i))

#define lua_equal(L,idx1,idx2)		lua_compare(L,(idx1),(idx2),LUA_OPEQ)
#define lua_lessthan(L,idx1,idx2)	lua_compare(L,(idx1),(idx2),LUA_OPLT)

#define LUA_COMPAT_MODULE

#endif				/* } */

/* }================================================================== */


/* a type with at least 32 bits */
#define LUA_INT32	int

/* limit on the size of the Lua stack; the pseudo-indices start below it */
#define LUAI_MAXSTACK		1000000

#define LUAI_FIRSTPSEUDOIDX	(-LUAI_MAXSTACK - 1000)

/* initial buffer size of the lauxlib buffer system (it needs <stdio.h>) */
#define LUAL_BUFFERSIZE		BUFSIZ


/*
** {==================================================================
** Number types: 'double' numbers, 'ptrdiff_t' integers and 32-bit
** unsigned integers (fixed: they are part of the ABI)
** ===================================================================
*/

#define LUA_NUMBER_DOUBLE
#define LUA_NUMBER	double

#define LUAI_UACNUMBER	double

#define LUA_NUMBER_SCAN		"%lf"
#define LUA_NUMBER_FMT		"%.14g"

#define LUA_INTEGER	ptrdiff_t

#define LUA_UNSIGNED	unsigned LUA_INT32

/* }================================================================== */


#endif
