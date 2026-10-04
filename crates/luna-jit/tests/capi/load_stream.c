/* how often lua_load calls the reader for a text chunk: the parser reads
   only as far as it has got, so it stops calling the reader at a syntax
   error, and reads the next piece as soon as it moves past the end of the
   one it has */
#include <stdio.h>
#include <string.h>
#include "lua.h"
#include "lauxlib.h"
#include "lualib.h"

#if LUA_VERSION_NUM == 501
#define LOAD(L, r, d, n) lua_load(L, r, d, n)
#else
#define LOAD(L, r, d, n) lua_load(L, r, d, n, NULL)
#endif

typedef struct {
  const char *const *pieces;
  int next;
  int calls;
} Pieces;

static const char *reader(lua_State *L, void *ud, size_t *size) {
  Pieces *p = (Pieces *)ud;
  const char *s = p->pieces[p->next];
  (void)L;
  p->calls++;
  if (s == NULL) {
    *size = 0;
    return NULL;
  }
  p->next++;
  *size = strlen(s);
  return s;
}

static void try_load(lua_State *L, const char *title, const char *const *pieces) {
  Pieces p;
  int st, n = 0;
  p.pieces = pieces;
  p.next = 0;
  p.calls = 0;
  while (pieces[n] != NULL) n++;
  st = LOAD(L, reader, &p, "=s");
  printf("%s: status %d, %d of %d pieces read, %d calls", title, st, p.next, n, p.calls);
  if (st != 0) printf(": %s", lua_tostring(L, -1));
  printf("\n");
  lua_settop(L, 0);
}

/* the program cut into one-byte pieces */
static void bytewise(lua_State *L, const char *title, const char *src) {
  static char bytes[512][2];
  static const char *pieces[513];
  size_t i, n = strlen(src);
  for (i = 0; i < n; i++) {
    bytes[i][0] = src[i];
    bytes[i][1] = '\0';
    pieces[i] = bytes[i];
  }
  pieces[n] = NULL;
  try_load(L, title, pieces);
}

#define LOAD_PIECES(title, ...)                               \
  do {                                                        \
    static const char *const ps[] = {__VA_ARGS__, NULL};      \
    try_load(L, title, ps);                                   \
  } while (0)

int main(void) {
  lua_State *L = luaL_newstate();
  luaL_openlibs(L);
  LOAD_PIECES("whole program", "local x", " = 1", "0 return x", "+1");
  LOAD_PIECES("error in the first piece", "x = = 1\n", "print(1)\n", "return 2");
  LOAD_PIECES("error token ends a piece", "x = )", "print(1)", "return 2");
  LOAD_PIECES("error token ends a piece, newline next", "x = )", "\nprint(1)", "return 2");
  LOAD_PIECES("numeral runs into the next piece", "local 1", "abc = 2", "return 3");
  LOAD_PIECES("keyword split", "ret", "urn 1", "\n");
  LOAD_PIECES("unfinished string", "x = 'abc", "def", "\n'", "return 1");
  LOAD_PIECES("long comment across pieces", "--[[ a", " b ]", "] return 5", "");
  LOAD_PIECES("long string unfinished", "x = [==[ a", " b ]=]", " c", "x = 1");
  LOAD_PIECES("minus at a piece end", "return 1 -", "- 2", "\n");
  LOAD_PIECES("break outside a loop", "break\n", "x = 1\n", "return x");
  LOAD_PIECES("missing end", "if x then\n", "y = 1\n", "else\n");
  LOAD_PIECES("empty pieces mid way", "return", "", " 1");
#if LUA_VERSION_NUM >= 504
  LOAD_PIECES("assign to const", "local x <const> = 1; x = 2\n", "print(1)\n", "return 3");
#endif
#if LUA_VERSION_NUM >= 502
  LOAD_PIECES("goto without a label", "do goto nowhere end\n", "x = 1\n", "return x");
#endif
  bytewise(L, "bytewise program",
           "local t = {1, 2, [3] = 'three', x = 4.5}\n"
           "local s = 0\n"
           "for i, v in ipairs(t) do s = s + i end -- comment\n"
           "return s .. [[long]] .. \"str\\n\"\n");
  bytewise(L, "bytewise error", "local a = {1, 2,, 3}\nreturn a\n");
  lua_close(L);
  return 0;
}
