-- Numeric for: which control value is checked first, how the error is
-- worded, whether a zero step is an error, when the loop is a float
-- loop, and how many times an extreme loop runs.
local ld = loadstring or load
local function e(src)
  local f = assert(ld(src, "=c"))
  print(pcall(f))
end
e("for i = 1, nil do end")
e("local x for i = 1, x do end")
e("for i = nil, 2 do end")
e("for i = 1, 2, nil do end")
e("for i = nil, nil, nil do end")
e("for i = 'a', 2 do end")
e("for i = 1, {} do end")
e("for i = 1, 2, {} do end")
e("local c = 0 for i = 1, 3, 0 do c = c + 1 if c > 5 then break end end return c")
e("local c = 0 for i = 3, 1, 0 do c = c + 1 if c > 5 then break end end return c")
e("local c = 0 for i = 1.5, 3, 0 do c = c + 1 if c > 5 then break end end return c")
e("for i = 1, nil, 0 do end")
e("local r = {} for i = '1', 3 do r[#r + 1] = tostring(i) end return table.concat(r, ' ')")
e("local r = {} for i = 1, 3, '1' do r[#r + 1] = tostring(i) end return table.concat(r, ' ')")
e("local r = {} for i = 1, '3' do r[#r + 1] = tostring(i) end return table.concat(r, ' ')")
e("local c = 0 for i = 1, 0/0 do c = c + 1 end return c")
e("local c = 0 for i = 1, 0/0, -1 do c = c + 1 if c > 5 then break end end return c")
e("local c = 0 for i = 1, -1/0, -1 do c = c + 1 if c > 5 then break end end return c")
e("local c = 0 for i = 1, 1/0 do c = c + 1 if c > 5 then break end end return c")
