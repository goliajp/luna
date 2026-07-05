-- v2.13 CORPUS-IV: float tostring spelling — PUC 5.5 two-stage
-- %.15g → round-trip check → %.17g (lobject.c tostringbuffFloat).
-- This matrix pins both stages and the looks-like-integer ".0".
print(0.1, 1/3, 2^0.5, 3.5)
print(math.pi)
print(100.0, 1e14, 1e15, 1e16)
print(2^53)
print(1e-5, 5e-324)
print(1.7976931348623157e308)
print(0.30000000000000004)
print(123456789.123456789)
print(-0.0, 0.0)
print(2.5e-10, 1.5e100)
