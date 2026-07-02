-- v2.10 CORPUS: iteration (ipairs deterministic; pairs by explicit key).
local t = {"one", "two", "three", "four"}
for i, v in ipairs(t) do print(i, v) end

-- explicit key iteration for pairs (avoid ordering diff)
local m = {}
m.a = 1; m.b = 2; m.c = 3
for _, k in ipairs({"a", "b", "c"}) do print(k, m[k]) end

-- next() iteration explicit
local keys = {}
for k in pairs({x=1, y=2, z=3}) do keys[#keys+1] = k end
table.sort(keys)
print(table.concat(keys, ","))

-- table.unpack
print(table.unpack({10, 20, 30}))
