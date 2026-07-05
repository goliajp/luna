-- v2.14 HD 5.3 seed: the manual retires bit32 in 5.3, but the
-- stock build ships LUA_COMPAT_5_2 which keeps it loaded — the
-- diff ground truth is the default build.
print(type(bit32))
print(bit32.band(0xF0, 0x3C))
print(bit32.lshift(1, 4))
