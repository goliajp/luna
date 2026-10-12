/* errno left behind by each C call PUC makes, starting from 99 */
#include <errno.h>
#ifdef _WIN32
#include <io.h>
#include <windows.h>
#else
#include <unistd.h>
#include <dlfcn.h>
#define _popen popen
#define _pclose pclose
#define _isatty isatty
#endif
#include <locale.h>
#include <math.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#define T(name, stmt) do { errno = 99; stmt; printf("%-36s %d\n", name, errno); } while (0)
#ifdef _WIN32
static void inval(const wchar_t *e, const wchar_t *f, const wchar_t *fi, unsigned l, uintptr_t r) {
    (void)e; (void)f; (void)fi; (void)l; (void)r; printf("[invalid parameter] ");
}
#endif
int main(void) {
    setvbuf(stdout, NULL, _IONBF, 0);
#ifdef _WIN32
    _set_invalid_parameter_handler(inval);
#endif
    volatile double z = 0, m1 = -1, two = 2, three = 3, big = 1000, ten = 10, half = 0.5, x;
    char buf[256]; FILE *f; char *e;
    T("strtod 1.5", x = strtod("1.5", &e));
    T("strtod 0x1p3", x = strtod("0x1p3", &e));
    T("strtod inf", x = strtod("inf", &e));
    T("strtod nan", x = strtod("nan", &e));
    T("strtod 1e999", x = strtod("1e999", &e));
    T("strtod -1e999", x = strtod("-1e999", &e));
    T("strtod 1e-999", x = strtod("1e-999", &e));
    T("strtod 4e-320", x = strtod("4e-320", &e));
    T("strtod 2.2250738585072011e-308", x = strtod("2.2250738585072011e-308", &e));
    T("strtod 0x1p-1074", x = strtod("0x1p-1074", &e));
    T("strtod 0x1p-1080", x = strtod("0x1p-1080", &e));
    T("strtod 0x1p2000", x = strtod("0x1p2000", &e));
    T("strtod abc", x = strtod("abc", &e));
    T("strtod empty", x = strtod("", &e));
    T("sscanf 1e999", sscanf("1e999", "%lf", &x));
    T("sscanf 1e-999", sscanf("1e-999", "%lf", &x));
    T("sscanf x", sscanf("x", "%lf", &x));
    T("log 2", x = log(two)); T("log 0", x = log(z)); T("log -1", x = log(m1));
    T("log10 0", x = log10(z)); T("log2 0", x = log2(z)); T("log1p -1", x = log1p(m1));
    T("exp 1", x = exp(half)); T("exp 1000", x = exp(big)); T("exp -1000", x = exp(-big));
    T("pow 2 3", x = pow(two, three)); T("pow 10 400", x = pow(ten, 400.0)); T("pow 10 -400", x = pow(ten, -400.0));
    T("pow 0 -1", x = pow(z, -1.0)); T("pow -1 0.5", x = pow(m1, half)); T("pow 0 0", x = pow(z, z));
    T("fmod 5 3", x = fmod(5.0, three)); T("fmod 1 0", x = fmod(1.0, z)); T("fmod inf 1", x = fmod(HUGE_VAL, 1.0));
    T("sqrt 2", x = sqrt(two)); T("sqrt -1", x = sqrt(m1));
    T("acos 2", x = acos(two)); T("asin 2", x = asin(two)); T("acos 0.5", x = acos(half));
    T("atan2 0 0", x = atan2(z, z)); T("sinh 1000", x = sinh(big)); T("cosh 1000", x = cosh(big)); T("tanh 1000", x = tanh(big));
    T("sin 1e300", x = sin(1e300)); T("sin inf", x = sin(HUGE_VAL)); T("cos inf", x = cos(HUGE_VAL)); T("tan inf", x = tan(HUGE_VAL));
    T("ldexp 1 5000", x = ldexp(1.0, 5000)); T("ldexp 1 -5000", x = ldexp(1.0, -5000)); T("ldexp 1 3", x = ldexp(1.0, 3));
    { int ex; T("frexp 8", x = frexp(8.0, &ex)); }
    { double ip; T("modf 1.5", x = modf(1.5, &ip)); }
    T("floor 1.5", x = floor(1.5)); T("ceil 1.5", x = ceil(1.5));
    T("fopen w ok", f = fopen("t.txt", "w")); T("fprintf", fprintf(f, "%s %.14g\n", "x", 1.5)); T("fwrite", fwrite("ab", 1, 2, f));
    T("fflush", fflush(f)); T("fseek ok", fseek(f, 0, SEEK_SET)); T("ftell", ftell(f)); T("fclose ok", fclose(f));
    T("fopen r ok", f = fopen("t.txt", "r")); T("setvbuf full", setvbuf(f, NULL, _IOFBF, 1024)); T("getc", getc(f)); T("ungetc", ungetc('x', f));
    T("fread", fread(buf, 1, 100, f)); T("fread eof", fread(buf, 1, 100, f)); T("feof/ferror", (void)(feof(f) + ferror(f)));
    T("fgets eof", fgets(buf, 10, f)); T("fscanf eof", fscanf(f, "%lf", &x)); T("clearerr", clearerr(f));
    T("fwrite on r", fwrite("ab", 1, 2, f)); T("fseek bad", fseek(f, -10, SEEK_SET)); T("fclose", fclose(f));
    T("fopen a ok", f = fopen("t.txt", "a")); T("fread on w", fread(buf, 1, 1, f)); fclose(f);
    T("fopen r+ ok", f = fopen("t.txt", "r+")); T("getc r+", getc(f)); T("fwrite after read", fwrite("Z", 1, 1, f)); fclose(f);
    T("fopen missing", f = fopen("no/such", "r")); T("fopen bad mode", f = fopen("t.txt", "q"));
    T("fopen dir", f = fopen(".", "r")); if (f) fclose(f);
    T("fopen ccs utf8", f = fopen("u.txt", "w, ccs=UTF-8")); if (f) fclose(f);
    T("fopen ccs bad", f = fopen("u.txt", "w, ccs=KOI8")); if (f) fclose(f);
    T("tmpfile", f = tmpfile()); T("fclose tmp", fclose(f));
    T("tmpnam", tmpnam(buf));
    T("remove ok", remove("t.txt")); T("remove missing", remove("t.txt"));
    f = fopen("a.txt", "w"); fclose(f);
    T("rename ok", rename("a.txt", "b.txt")); T("rename missing", rename("a.txt", "c.txt")); remove("b.txt");
    T("system exit 3", system("exit 3")); T("system NULL", system(NULL));
    T("_popen", f = _popen("exit 3", "r")); T("fread popen", fread(buf, 1, 10, f)); T("_pclose", _pclose(f));
    T("_popen w", f = _popen("cat", "w")); T("_pclose w", _pclose(f));
    T("getenv set", getenv("PATH")); T("getenv unset", getenv("NO_SUCH_VAR_X"));
    T("time", time(NULL)); { time_t t0 = 0; T("localtime", localtime(&t0)); T("gmtime", gmtime(&t0));
      struct tm tm = *gmtime(&t0); T("mktime ok", mktime(&tm)); tm.tm_year = 100000000; T("mktime huge", mktime(&tm));
      T("strftime %Y", strftime(buf, 100, "%Y", gmtime(&t0))); T("strftime %c", strftime(buf, 100, "%c", gmtime(&t0)));
      T("strftime empty out", strftime(buf, 100, "%p", gmtime(&t0))); }
    T("clock", clock());
    T("setlocale query", setlocale(LC_ALL, NULL)); T("setlocale C", setlocale(LC_ALL, "C")); T("setlocale bad", setlocale(LC_ALL, "xx-yy"));
    T("_isatty 0", _isatty(0)); T("_isatty 1", _isatty(1)); T("_isatty 2", _isatty(2));
    T("malloc", free(malloc(100))); T("realloc", free(realloc(NULL, 100)));
#ifdef _WIN32
    T("LoadLibraryExA missing", LoadLibraryExA("no_such.dll", NULL, LOAD_WITH_ALTERED_SEARCH_PATH));
#else
    T("dlopen missing", dlopen("no_such.so", RTLD_NOW));
#endif
    T("printf", printf("%s", "")); T("fputs stderr", fputs("", stderr)); T("fflush stdout", fflush(stdout));
    T("snprintf %.14g", snprintf(buf, 100, "%.14g", 1.5)); T("snprintf %a", snprintf(buf, 100, "%a", 1.5));
    T("strcoll", strcoll("a", "b")); T("strerror", strerror(2));
    T("memchr", memchr("abc", 'c', 3));
    return 0;
}
