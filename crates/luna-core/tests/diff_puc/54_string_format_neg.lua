-- v2.11 CORPUS-II: string.format negative + width interactions.
print(string.format("%d", -42))
print(string.format("%5d", -42))
print(string.format("%-5d|", -42))
print(string.format("%05d", -42))     -- 0-pad with sign
print(string.format("%+5d", -42))
print(string.format("%+5d", 42))
print(string.format("%.3d", 7))       -- min-digits precision
print(string.format("%.3d", 12345))
-- C-style "%*d" dynamic width is NOT valid in Lua — errors.
print((pcall(string.format, "%*d", 6, 42)))
