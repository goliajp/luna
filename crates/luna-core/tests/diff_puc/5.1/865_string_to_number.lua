-- String to number conversion, which arithmetic, for loops and
-- tonumber share: 5.1 runs C strtod (inf/nan accepted, a NUL ends the
-- string), 5.2 rejects those, and both only ever produce floats.
local function show(x) return x ~= x and "nan" or string.format("%.17g", x) end
print(show("inf" + 0), show("-inf" * 1), show("nan" + 0), show("  0x10  " + 0))
print(tonumber("inf"), tonumber("nan") ~= tonumber("nan"), tonumber("10\0zz"), tonumber("0x1p4"))
print("10\0zz" + 1)
print(show("9007199254740993" + 0), show("5" % 0))
for i = "1", 2 do io.write(show(i), " ") end
print()
