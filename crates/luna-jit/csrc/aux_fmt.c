/*
 * lua_pushfstring and lua_pushvfstring, after each version's
 * luaO_pushvfstring: %% %s %c %d %f %p everywhere, %I and %U from 5.3 on.
 * An unknown option is copied as it is in 5.1 and 5.5, and an error in
 * 5.2 to 5.4. Numbers are written as the dialect writes them.
 */
#include <stdlib.h>
#include <string.h>
#include "auxlib.h"

/* the result being built; starts in `space`, moves to the heap when it
   outgrows it */
struct fbuf {
  char *b;
  size_t n, size;
  char space[256];
};

static void fb_add(struct fbuf *fb, const char *s, size_t l) {
  if (fb->size - fb->n < l) {
    size_t nsize = (fb->size + l) * 2;
    char *nb = (char *)malloc(nsize);
    if (nb == NULL) abort();
    memcpy(nb, fb->b, fb->n);
    if (fb->b != fb->space) free(fb->b);
    fb->b = nb;
    fb->size = nsize;
  }
  memcpy(fb->b + fb->n, s, l);
  fb->n += l;
}

static void fb_free(struct fbuf *fb) {
  if (fb->b != fb->space) free(fb->b);
}

static void fb_num(struct fbuf *fb, lua_State *L, int isint, lua_Integer i, lua_Number n) {
  char buff[64];
  size_t l = luna_capi_num2str(L, isint, i, n, buff);
  fb_add(fb, buff, l);
}

/* PUC luaO_utf8esc: the UTF-8 bytes of x end at buff[7]; returns their
   count */
static int utf8esc(char *buff, unsigned long x) {
  int n = 1;
  if (x < 0x80)
    buff[7] = (char)x;
  else {
    unsigned int mfb = 0x3f;
    do {
      buff[8 - (n++)] = (char)(0x80 | (x & 0x3f));
      x >>= 6;
      mfb >>= 1;
    } while (x > mfb);
    buff[8 - n] = (char)((~mfb << 1) | x);
  }
  return n;
}

/* 5.3's %c of a byte its lctype does not call printable */
static int printable(unsigned char c) {
  return c >= 0x20 && c < 0x7f;
}

LUNA_HIDDEN const char *luna_c_lua_pushvfstring(lua_State *L, const char *fmt,
                                                va_list argp) {
  int v = VNUM(L);
  struct fbuf fb;
  const char *e;
  fb.b = fb.space;
  fb.n = 0;
  fb.size = sizeof(fb.space);
  while ((e = strchr(fmt, '%')) != NULL) {
    fb_add(&fb, fmt, (size_t)(e - fmt));
    switch (e[1]) {
      case 's': {
        const char *s = va_arg(argp, char *);
        if (s == NULL) s = "(null)";
        fb_add(&fb, s, strlen(s));
        break;
      }
      case 'c': {
        unsigned char c = (unsigned char)va_arg(argp, int);
        if (v == 503 && !printable(c)) {
          char buff[16];
          int l = snprintf(buff, sizeof(buff), "<\\%d>", (int)c);
          fb_add(&fb, buff, (size_t)l);
        }
        else if (v != 501 || c != 0)
          fb_add(&fb, (const char *)&c, 1);
        break;
      }
      case 'd':
        fb_num(&fb, L, 1, va_arg(argp, int), 0);
        break;
      case 'f':
        fb_num(&fb, L, 0, 0, (lua_Number)va_arg(argp, double));
        break;
      case 'p': {
        char buff[64];
        int l = snprintf(buff, sizeof(buff), "%p", va_arg(argp, void *));
        fb_add(&fb, buff, (size_t)l);
        break;
      }
      case '%':
        fb_add(&fb, "%", 1);
        break;
      case 'I':
        if (v >= 503) {
          fb_num(&fb, L, 1, (lua_Integer)va_arg(argp, long long), 0);
          break;
        }
        goto unknown;
      case 'U':
        if (v >= 503) {
          char buff[8];
          unsigned long x = v >= 505 ? (unsigned long)(uint32_t)va_arg(argp, unsigned long)
                                     : (unsigned long)va_arg(argp, long);
          int l = utf8esc(buff, x);
          fb_add(&fb, buff + 8 - l, (size_t)l);
          break;
        }
        goto unknown;
      default:
      unknown:
        if (v == 501 || v == 505)
          fb_add(&fb, e, 2);
        else {
          char msg[64];
          snprintf(msg, sizeof(msg), "invalid option '%%%c' to 'lua_pushfstring'", e[1]);
          fb_free(&fb);
          lua_pushstring(L, msg);
          lua_error(L);
        }
        break;
    }
    fmt = e + 2;
  }
  fb_add(&fb, fmt, strlen(fmt));
  {
    const char *r = lua_pushlstring(L, fb.b, fb.n);
    fb_free(&fb);
    return r;
  }
}

LUNA_HIDDEN const char *luna_c_lua_pushfstring(lua_State *L, const char *fmt, ...) {
  const char *r;
  va_list argp;
  va_start(argp, fmt);
  r = luna_c_lua_pushvfstring(L, fmt, argp);
  va_end(argp);
  return r;
}
