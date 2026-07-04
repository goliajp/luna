-- v2.14 CV.3: goto over nested loops + continue idiom.
local out = {}
for i = 1, 3 do
  for j = 1, 3 do
    if j == 2 then goto next_j end
    if i == 3 then goto done end
    out[#out + 1] = i .. ":" .. j
    ::next_j::
  end
end
::done::
print(table.concat(out, " "))
