/* lua_pushfstring and lua_pushvfstring: every option of each version,
   unknown options, long results, and the stack they leave */
#include <stdarg.h>
#include <math.h>
#include "aux_common.h"

static const char *vf(lua_State *L, const char *fmt, ...) {
  const char *r;
  va_list ap;
  va_start(ap, fmt);
  r = lua_pushvfstring(L, fmt, ap);
  va_end(ap);
  return r;
}

static void check(lua_State *L, const char *label, const char *r) {
  size_t len;
  const char *s = lua_tolstring(L, -1, &len);
  printf("%s: top=%d same=%d len=%d [", label, lua_gettop(L), r == s, (int)len);
  put_bytes(s, len);
  printf("]\n");
  lua_settop(L, 0);
}

static int bad_option(lua_State *L) {
  lua_pushinteger(L, 1);
  lua_pushfstring(L, "a %q b", 5);
  printf("  bad_option went on: top=%d\n", lua_gettop(L));
  return 1;
}

static int bad_trailing(lua_State *L) {
  lua_pushfstring(L, "x %y");
  return 1;
}

#if LUA_VERSION_NUM >= 503
static int bad_I(lua_State *L) {
  lua_pushfstring(L, "%I", (LUAI_UACINT)7);
  return 1;
}
#else
static int bad_I(lua_State *L) {
  lua_pushfstring(L, "<%I>");
  return 1;
}
#endif

int main(void) {
  lua_State *L = luaL_newstate();
  char big[3000];
  int i;
  check(L, "plain", lua_pushfstring(L, "hello"));
  check(L, "empty", lua_pushfstring(L, ""));
  check(L, "percent", lua_pushfstring(L, "100%% sure %%"));
  check(L, "s", lua_pushfstring(L, "<%s|%s>", "abc", ""));
  check(L, "s null", lua_pushfstring(L, "<%s>", (char *)NULL));
  check(L, "c", lua_pushfstring(L, "<%c%c%c>", 'a', ' ', '~'));
  check(L, "c ctrl", lua_pushfstring(L, "<%c>", 5));
  check(L, "c high", lua_pushfstring(L, "<%c>", 200));
  check(L, "c zero", lua_pushfstring(L, "<%c>", 0));
  check(L, "d", lua_pushfstring(L, "%d %d %d", 0, -42, 2147483647));
  check(L, "d min", lua_pushfstring(L, "%d", (int)(-2147483647 - 1)));
  check(L, "f", lua_pushfstring(L, "%f %f %f", 1.5, 1.0, -0.25));
  check(L, "f big", lua_pushfstring(L, "%f %f", 1e100, 123456789012.0));
  check(L, "f small", lua_pushfstring(L, "%f %f", 1e-10, 0.1));
  check(L, "f int-like", lua_pushfstring(L, "%f %f", 3.0, 1e15));
  check(L, "f inf", lua_pushfstring(L, "%f %f", HUGE_VAL, -HUGE_VAL));
#if LUA_VERSION_NUM >= 503
  check(L, "I", lua_pushfstring(L, "%I %I", (LUAI_UACINT)0, (LUAI_UACINT)LUA_MININTEGER));
  check(L, "I max", lua_pushfstring(L, "%I", (LUAI_UACINT)LUA_MAXINTEGER));
  check(L, "U ascii", lua_pushfstring(L, "%U", (long)0x41));
  check(L, "U 2", lua_pushfstring(L, "%U", (long)0xE9));
  check(L, "U 3", lua_pushfstring(L, "%U", (long)0x20AC));
  check(L, "U 4", lua_pushfstring(L, "%U", (long)0x10FFFF));
  check(L, "U 5", lua_pushfstring(L, "%U", (long)0x3FFFFFF));
  check(L, "U 6", lua_pushfstring(L, "%U", (long)0x7FFFFFFF));
  check(L, "U 0", lua_pushfstring(L, "%U", (long)0));
#endif
  {
    int x;
    const char *r = lua_pushfstring(L, "%p", (void *)&x);
    printf("p: top=%d nonempty=%d\n", lua_gettop(L), r[0] != '\0');
    lua_settop(L, 0);
  }
  check(L, "mixed", lua_pushfstring(L, "%s=%d (%f)%%%c", "k", 7, 2.5, '!'));
  check(L, "vf", vf(L, "%s-%d-%f", "v", 3, 0.5));
  check(L, "vf empty", vf(L, ""));
  for (i = 0; i < (int)sizeof(big) - 1; i++) big[i] = (char)('a' + i % 26);
  big[sizeof(big) - 1] = '\0';
  {
    const char *r = lua_pushfstring(L, "[%s][%s]", big, big);
    size_t len;
    lua_tolstring(L, -1, &len);
    printf("long: top=%d len=%d same=%d head=%.5s\n", lua_gettop(L), (int)len,
           r == lua_tostring(L, -1), r);
    lua_settop(L, 0);
  }
  /* the result goes on top of what is there */
  lua_pushinteger(L, 1);
  lua_pushstring(L, "below");
  lua_pushfstring(L, "%s", "x");
  show_from(L, "stack", 1);
  lua_settop(L, 0);
  /* unknown options: copied (5.1, 5.5) or an error (5.2 to 5.4) */
  run(L, "bad option", bad_option, NULL);
  run(L, "bad trailing", bad_trailing, NULL);
  run(L, "I option", bad_I, NULL);
  lua_close(L);
  return 0;
}
