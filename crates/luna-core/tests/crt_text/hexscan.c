#include <stdio.h>
#include <stdint.h>
#include <string.h>

static uint64_t s = 0x9e3779b97f4a7c15ull;
static uint64_t rnd(void) { s ^= s << 13; s ^= s >> 7; s ^= s << 17; return s; }

static void emit(const char *in) {
    double d = 0; uint64_t b;
    int n = sscanf(in, "%lf", &d);
    memcpy(&b, &d, 8);
    printf("%s\t%d\t%016llx\n", in, n, (unsigned long long)b);
}

int main(void) {
    static const char *fixed[] = {
        "0x1.00000000000008p0", "0x1.00000000000018p0", "0x1.000000000000080000001p0",
        "0x1.fffffffffffff8p0", "0x1.fffffffffffff7ffffffp0", "0x1.fffffffffffffcp1023",
        "0x1.fffffffffffff8p1023", "0x1p-1074", "0x1p-1075", "0x1.0000000000001p-1075",
        "0x1.8p-1075", "0x1p-1076", "0x0.0000000000001p-1022", "0x0.00000000000008p-1022",
        "0x0.00000000000018p-1022", "0x1.ffffffffffffffffffffffffp-1023", "0x123456789abcdef0123p0",
        "0x123456789abcdef08p0", "0x123456789abcdef18p0", "0x.8000000000000080000p1",
        "0x1p1024", "0x1p-2000", "0x0p0", "-0x1.00000000000008p0", "0xFFFFFFFFFFFFFFFFFFFFp-80",
    };
    for (size_t i = 0; i < sizeof fixed / sizeof fixed[0]; i++) emit(fixed[i]);
    static const char hx[] = "0123456789abcdef";
    char buf[128];
    for (int k = 0; k < 4000; k++) {
        int len = 1 + rnd() % 40, p = 0;
        if (rnd() % 4 == 0) buf[p++] = '-';
        buf[p++] = '0'; buf[p++] = 'x';
        int dot = (rnd() % 3 == 0) ? (int)(rnd() % (len + 1)) : -1;
        for (int j = 0; j < len; j++) {
            if (j == dot) buf[p++] = '.';
            int r = rnd() % 10;
            buf[p++] = r < 3 ? hx[rnd() % 16] : (r < 6 ? (j == 0 ? '1' : '0') : (r < 8 ? 'f' : '8'));
        }
        int e = (int)(rnd() % 2300) - 1150;
        p += sprintf(buf + p, "p%d", e);
        buf[p] = 0;
        emit(buf);
    }
    static const char *dfixed[] = {
        "2.2250738585072011e-308", "2.2250738585072012e-308", "4.9406564584124654e-324",
        "2.4703282292062327e-324", "2.4703282292062328e-324", "1.7976931348623157e308",
        "1.7976931348623158e308", "1.7976931348623159e308", "9007199254740993",
        "9007199254740992.5", "0.1", "1e23", "8.5e-321",
        "179769313486231580793728971405303415079934132710037826936173778980444968292764750946649017977587207096330286416692887910946555547851940402630657488671505820681908902000708383676273854845817711531764475730270069855571366959622842914819860834936475292719074168444365510704342711559699508093042880177904174497791",
    };
    for (size_t i = 0; i < sizeof dfixed / sizeof dfixed[0]; i++) emit(dfixed[i]);
    for (int k = 0; k < 3000; k++) {
        int len = 1 + rnd() % 30, p = 0;
        if (rnd() % 4 == 0) buf[p++] = '-';
        int dot = (rnd() % 2 == 0) ? (int)(rnd() % (len + 1)) : -1;
        for (int j = 0; j < len; j++) {
            if (j == dot) buf[p++] = '.';
            int r = rnd() % 10;
            buf[p++] = r < 5 ? (char)('0' + rnd() % 10) : (r < 7 ? '9' : (r < 9 ? '0' : '5'));
        }
        if (dot == len) buf[p++] = '0';
        int e = (int)(rnd() % 700) - 350;
        p += sprintf(buf + p, "e%d", e);
        buf[p] = 0;
        emit(buf);
    }
    return 0;
}
