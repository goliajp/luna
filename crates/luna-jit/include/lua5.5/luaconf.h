/*
** luna's luaconf.h for the Lua 5.5 dialect: the configuration of a
** default 64-bit build, as PUC-Rio's luaconf.h gives it.
** See Copyright Notice in lua.h
*/


#ifndef luaconf_h
#define luaconf_h

#include <limits.h>
#include <stddef.h>


/*
** {==================================================================
** Number types: 'long long' integers and 'double' floats (fixed: they
** are part of the ABI)
** ===================================================================
*/

#define LUA_INT_INT		1
#define LUA_INT_LONG		2
#define LUA_INT_LONGLONG	3

#define LUA_FLOAT_FLOAT		1
#define LUA_FLOAT_DOUBLE	2
#define LUA_FLOAT_LONGDOUBLE	3

#define LUA_INT_DEFAULT		LUA_INT_LONGLONG
#define LUA_FLOAT_DEFAULT	LUA_FLOAT_DOUBLE

#define LUA_C89_NUMBERS		0

#define LUA_INT_TYPE	LUA_INT_DEFAULT
#define LUA_FLOAT_TYPE	LUA_FLOAT_DEFAULT

/* }================================================================== */


/*
** {==================================================================
** Paths
** ===================================================================
*/

#define LUA_PATH_SEP            ";"
#define LUA_PATH_MARK           "?"
#define LUA_EXEC_DIR            "!"

/* no install prefix: only the current-directory templates */
#if !defined(LUA_PATH_DEFAULT)
#define LUA_PATH_DEFAULT	"./?.lua;./?/init.lua"
#endif

#if !defined(LUA_CPATH_DEFAULT)
#define LUA_CPATH_DEFAULT	""
#endif

#if !defined(LUA_DIRSEP)

#if defined(_WIN32)
#define LUA_DIRSEP	"\\"
#else
#define LUA_DIRSEP	"/"
#endif

#endif

#define LUA_IGMARK		"-"

/* }================================================================== */


/*
** {==================================================================
** Marks for exported symbols
** ===================================================================
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
#if defined(__cplusplus)
/* Lua uses the "C name" when calling open functions */
#define LUAMOD_API	extern "C"
#else
#define LUAMOD_API	LUA_API
#endif

/* }================================================================== */


/*
** {==================================================================
** Compatibility with previous versions
** ===================================================================
*/

/* LUA_COMPAT_GLOBAL avoids 'global' being a reserved word */
#if !defined(LUA_COMPAT_GLOBAL)
#define LUA_COMPAT_GLOBAL	1
#endif

#define lua_strlen(L,i)		lua_rawlen(L, (i))

#define lua_objlen(L,i)		lua_rawlen(L, (i))

#define lua_equal(L,idx1,idx2)		lua_compare(L,(idx1),(idx2),LUA_OPEQ)
#define lua_lessthan(L,idx1,idx2)	lua_compare(L,(idx1),(idx2),LUA_OPLT)

/* }================================================================== */


/*
** {==================================================================
** Configuration for numbers
** ===================================================================
*/

#define LUA_NUMBER	double

#define LUAI_UACNUMBER	double

#define LUA_NUMBER_FRMLEN	""
#define LUA_NUMBER_FMT		"%.15g"
#define LUA_NUMBER_FMT_N	"%.17g"


#define LUA_INTEGER_FMT		"%" LUA_INTEGER_FRMLEN "d"

#define LUAI_UACINT		LUA_INTEGER

#define LUA_UNSIGNED		unsigned LUAI_UACINT

#define LUA_INTEGER		long long
#define LUA_INTEGER_FRMLEN	"ll"

#define LUA_MAXINTEGER		LLONG_MAX
#define LUA_MININTEGER		LLONG_MIN

#define LUA_MAXUNSIGNED		ULLONG_MAX

/* }================================================================== */


/*
** {==================================================================
** Dependencies with C99 and other C details
** ===================================================================
*/

/*
@@ LUA_KCONTEXT is the type of the context ('ctx') for continuation
** functions: 'intptr_t' if available, otherwise 'ptrdiff_t'
*/
#define LUA_KCONTEXT	ptrdiff_t

#if !defined(LUA_USE_C89) && defined(__STDC_VERSION__) && \
    __STDC_VERSION__ >= 199901L
#include <stdint.h>
#if defined(INTPTR_MAX)  /* even in C99 this type is optional */
#undef LUA_KCONTEXT
#define LUA_KCONTEXT	intptr_t
#endif
#endif


/*
** macros to improve jump prediction (some macros of the API use them;
** define LUA_NOBUILTIN to avoid '__builtin_expect')
*/
#if !defined(luai_likely)

#if !defined(LUA_NOBUILTIN) && defined(__GNUC__) && (__GNUC__ >= 3)
#define luai_likely(x)		(__builtin_expect(((x) != 0), 1))
#define luai_unlikely(x)	(__builtin_expect(((x) != 0), 0))
#else
#define luai_likely(x)		(x)
#define luai_unlikely(x)	(x)
#endif

#endif

/* }================================================================== */


/*
** {==================================================================
** Macros that affect the API and must be the same when you compile Lua
** and when you compile code that links to Lua
** =====================================================================
*/

#define LUA_EXTRASPACE		(sizeof(void *))

#define LUA_IDSIZE	60

#define LUAL_BUFFERSIZE   ((int)(16 * sizeof(void*) * sizeof(lua_Number)))

#if defined(LLONG_MAX)
/* use ISO C99 stuff */
#define LUAI_MAXALIGN long double u; void *s; long long l
#else
/* use only C89 stuff */
#define LUAI_MAXALIGN  lua_Number n; double u; void *s; lua_Integer i; long l
#endif

/* }================================================================== */

#endif
