-- v2.12 CORPUS-III: string.dump + load roundtrip.
local f = function(a, b) return a * 100 + b end
local dumped = string.dump(f)
print(type(dumped), #dumped > 0)
local g, err = load(dumped)
if g then print(g(3, 7)) end
