-- v2.15 P2.4 utf8: codes() iterator basic.
local s = "abc"
local out = {}
for pos, cp in utf8.codes(s) do
  out[#out+1] = tostring(pos) .. "=" .. cp
end
print(table.concat(out, ","))
