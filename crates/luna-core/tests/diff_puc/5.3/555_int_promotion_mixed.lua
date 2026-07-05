-- v2.15 P2.5 (5.3): mixed int/float promotion.
local i = 3
local f = 4.0
print(math.type(i + f))    -- float (contagion)
print(math.type(i * f))
print(math.type(i / f))     -- float (div always)
print(math.type(i - i))     -- integer
print(math.type(i // f))    -- float (both float now)
