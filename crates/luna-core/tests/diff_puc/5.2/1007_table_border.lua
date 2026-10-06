-- `#t` on tables with holes: the border each PUC version picks depends on
-- how large the array part is and, from 5.4 on, on what earlier lookups
-- left behind; the construction history here decides both
local function r0() end
local function r1(x) return x end
local function r2(x) return x, x end
local function pk(...) return {...} end
local unpack = table.unpack or unpack
local function show(name, t) print(name, #t) end

show("calls", {r1(6), r1(7), r0(), r1(8)})
show("nil middle", {1, nil, 3})
show("nil lead", {nil, nil, 3})
show("nil tail", {1, 2, nil})
show("hole", {1, 2, nil, 4})
show("holes", {1, nil, nil, nil, 5, 6, nil, 8, nil})
show("vararg", pk(1, nil, 3, nil, 5))
show("call cut", {r2(1), x = 1})
show("call open", {r0(), r2(1)})
show("keyed", {[1] = 1, [3] = 3})
show("keyed after list", {1, 2, [4] = 4, [3] = nil})

local t = {}
for i = 1, 10 do t[i] = i end
t[5] = nil; show("fill 10, drop 5", t)
t[10] = nil; show("drop 10", t)
t[9] = nil; t[8] = nil; show("drop 9 8", t)
t[6] = nil; show("drop 6", t)
t[20] = 1; show("hash 20", t)

t = {}
for i = 1, 17 do t[i] = i end
for i = 2, 16, 2 do t[i] = nil end
show("odd of 17", t)
t[17] = nil; show("odd of 16", t)

t = {}
t[1] = 1; t[3] = 3; show("1 3", t)
t[4] = 4; show("1 3 4", t)
t[2] = 2; show("1 2 3 4", t)
t[6] = 6; t[8] = 8; show("1 2 3 4 6 8", t)
t[4] = nil; show("drop 4", t)

t = {1, 2, 3, 4, 5, 6, 7, 8}
table.remove(t); table.remove(t); show("remove twice", t)
t[3] = nil; show("hole 3", t)
local _ = t[6]; show("read 6", t)
table.insert(t, 9); show("insert", t)
table.insert(t, 1, 0); show("insert front", t)
t[8] = nil; t[9] = nil; show("drop 8 9", t)
for _ in ipairs(t) do end; show("ipairs", t)

t = {}
for i = 1, 33 do t[i] = i end
t[33] = nil; t[20] = nil; show("33 minus 33 20", t)
t[17] = nil; show("minus 17", t)
t[40] = 40; t[41] = 41; show("hash 40 41", t)

-- the same in loops, so that the trace and method JITs run them
local function churn(n)
  local t, acc = {}, {}
  for i = 1, n do
    t[i] = i
    if i % 3 == 0 then t[i - 1] = nil end
    if i % 5 == 0 then local _ = t[i + 2] end
    acc[#acc + 1] = #t
  end
  return table.concat(acc, " ")
end
print("churn", churn(70))

local function ctor(n)
  local acc = {}
  for i = 1, n do
    local u = {i, nil, i, r0()}
    local v = {r1(i), r0(), r1(i)}
    local w = pk(i, nil, i, nil)
    acc[#acc + 1] = #u .. "/" .. #v .. "/" .. #w
  end
  return table.concat(acc, " ")
end
print("ctor", ctor(12))

local function fill(n)
  local t = {}
  for i = 1, n do t[i] = i end
  local acc = {}
  for i = n, 1, -3 do
    t[i] = nil
    acc[#acc + 1] = #t
  end
  for i = 1, n, 4 do
    table.insert(t, i)
    acc[#acc + 1] = #t
    table.remove(t)
    acc[#acc + 1] = #t
  end
  return table.concat(acc, " ")
end
print("fill", fill(100))

local function sparse(n)
  local t, acc = {}, {}
  for i = 1, n do
    t[i * 2] = i
    if i % 4 ~= 2 then t[i] = i end
    for _ in ipairs(t) do end
    acc[#acc + 1] = #t
  end
  return table.concat(acc, " ")
end
print("sparse", sparse(60))

-- generated histories (each prints #t after every step)
local L = {}
do local t = pk(4, 9, 3, 5, 9, r1(1), nil, 8, nil, nil, 1, r1(2), 4, r0(), nil, r1(6), 4, 5, 2, 5, 9, r2(1), r1(5), 4, 1, 2, 5, 1, r1(1), 1, 7, 4, r1(5), 5, 7, r1(2), 2, 8, r1(3), nil, 9, 3, 7, 7, r2(1), 5, r2(1), 3, 2, 3, 5); L[#L+1] = '0 c '..#t
  do local n = #t; if n > 0 then table.remove(t, (18 % n) + 1) end end; L[#L+1] = '0.0 '..#t
  t[3] = nil; L[#L+1] = '0.1 '..#t
  t[16] = 48; L[#L+1] = '0.2 '..#t
  t[5] = nil; L[#L+1] = '0.3 '..#t
  for _ in ipairs(t) do end; L[#L+1] = '0.4 '..#t
end
do local t = {}; L[#L+1] = '1 c '..#t
  table.remove(t); L[#L+1] = '1.0 '..#t
  t[25] = nil; L[#L+1] = '1.1 '..#t
  table.insert(t, 1); L[#L+1] = '1.2 '..#t
  t[3] = 33; L[#L+1] = '1.3 '..#t
  t[26] = 77; L[#L+1] = '1.4 '..#t
  t[#t + 1] = 93; L[#L+1] = '1.5 '..#t
  for _ in ipairs(t) do end; L[#L+1] = '1.6 '..#t
  t[36] = 79; L[#L+1] = '1.7 '..#t
  table.insert(t, 4); L[#L+1] = '1.8 '..#t
  X = t[14]; L[#L+1] = '1.9 '..#t
  do local n = #t; if n > 0 then table.remove(t, (17 % n) + 1) end end; L[#L+1] = '1.10 '..#t
  t[2] = nil; L[#L+1] = '1.11 '..#t
  t[19] = 74; L[#L+1] = '1.12 '..#t
  for _ in ipairs(t) do end; L[#L+1] = '1.13 '..#t
  t[11] = nil; L[#L+1] = '1.14 '..#t
end
do local t = {nil, r0(), 6, nil, r2(1), unpack({})}; L[#L+1] = '2 c '..#t
  t[#t + 1] = 10; L[#L+1] = '2.0 '..#t
  t[35] = 6; L[#L+1] = '2.1 '..#t
  X = t[9]; L[#L+1] = '2.2 '..#t
  t[6.5] = 1; L[#L+1] = '2.3 '..#t
  t[31] = nil; L[#L+1] = '2.4 '..#t
  t[1.5] = 1; L[#L+1] = '2.5 '..#t
  t[1.5] = 1; L[#L+1] = '2.6 '..#t
  do local n = #t; table.insert(t, n > 0 and (24 % (n + 1)) + 1 or 1, 1) end; L[#L+1] = '2.7 '..#t
  table.insert(t, 2); L[#L+1] = '2.8 '..#t
end
do local t = {7, r2(1), r2(1), 6, unpack({7, r2(1)})}; L[#L+1] = '4 c '..#t
  t[6] = 44; L[#L+1] = '4.0 '..#t
  table.insert(t, 8); L[#L+1] = '4.1 '..#t
  X = t[14]; L[#L+1] = '4.2 '..#t
  t[7.5] = 1; L[#L+1] = '4.3 '..#t
  t[9.5] = 1; L[#L+1] = '4.4 '..#t
  t[9] = 67; L[#L+1] = '4.5 '..#t
end
do local t = {r0(), 2, r1(8), 5, r1(9), nil, 9, 7, r2(1), r0(), nil, 3, r2(1), 8, 9, 1, nil, 1, 7, 2, 3, 9, 7, 8, 6, 8, 2, 1, r0(), 1, 3, 5, 6, 8, 2, r0(), nil, nil, nil, r0(), 7, r1(3), 9, nil, 9, nil, r0(), 6, r0(), 6, r0(), unpack({r0(), 2, r1(8), 5, r1(9), nil, 9, 7, r2(1), r0(), nil, 3})}; L[#L+1] = '5 c '..#t
  do local n = #t; if n > 0 then table.remove(t, (6 % n) + 1) end end; L[#L+1] = '5.0 '..#t
  t[16] = 12; L[#L+1] = '5.1 '..#t
  t[7] = 54; L[#L+1] = '5.2 '..#t
  t[#t + 1] = 81; L[#L+1] = '5.3 '..#t
  table.remove(t); L[#L+1] = '5.4 '..#t
  t[37] = 47; L[#L+1] = '5.5 '..#t
  t[33] = 44; L[#L+1] = '5.6 '..#t
  t[#t + 1] = nil; L[#L+1] = '5.7 '..#t
  t[#t + 1] = 71; L[#L+1] = '5.8 '..#t
  table.insert(t, 8); L[#L+1] = '5.9 '..#t
  t[12] = 23; L[#L+1] = '5.10 '..#t
  t[2.5] = 1; L[#L+1] = '5.11 '..#t
  table.insert(t, 8); L[#L+1] = '5.12 '..#t
  t[7] = 60; L[#L+1] = '5.13 '..#t
  t[14] = nil; L[#L+1] = '5.14 '..#t
  t[24] = 90; L[#L+1] = '5.15 '..#t
  t[13] = 59; L[#L+1] = '5.16 '..#t
  t[13] = nil; L[#L+1] = '5.17 '..#t
end
do local t = {1, 2, r1(7), nil, 6, r1(5), r1(5)}; L[#L+1] = '6 c '..#t
  do local n = #t; if n > 0 then table.remove(t, (25 % n) + 1) end end; L[#L+1] = '6.0 '..#t
  t[22] = nil; L[#L+1] = '6.1 '..#t
  table.remove(t); L[#L+1] = '6.2 '..#t
  do local n = #t; if n > 0 then table.remove(t, (13 % n) + 1) end end; L[#L+1] = '6.3 '..#t
  table.remove(t); L[#L+1] = '6.4 '..#t
  t[8.5] = 1; L[#L+1] = '6.5 '..#t
  t[23] = nil; L[#L+1] = '6.6 '..#t
  X = t[4]; L[#L+1] = '6.7 '..#t
  t[20] = 85; L[#L+1] = '6.8 '..#t
  t[18] = nil; L[#L+1] = '6.9 '..#t
  t[33] = nil; L[#L+1] = '6.10 '..#t
  do local n = #t; table.insert(t, n > 0 and (48 % (n + 1)) + 1 or 1, 7) end; L[#L+1] = '6.11 '..#t
end
do local t = {}; L[#L+1] = '7 c '..#t
  t[26] = 98; L[#L+1] = '7.0 '..#t
  t[25] = 34; L[#L+1] = '7.1 '..#t
  t[22] = 66; L[#L+1] = '7.2 '..#t
  t[19] = nil; L[#L+1] = '7.3 '..#t
  t[8.5] = 1; L[#L+1] = '7.4 '..#t
  t[9.5] = 1; L[#L+1] = '7.5 '..#t
  table.insert(t, 7); L[#L+1] = '7.6 '..#t
  X = t[11]; L[#L+1] = '7.7 '..#t
  t[16] = nil; L[#L+1] = '7.8 '..#t
  t[#t + 1] = 64; L[#L+1] = '7.9 '..#t
  t[13] = nil; L[#L+1] = '7.10 '..#t
  table.insert(t, 9); L[#L+1] = '7.11 '..#t
  t[18] = 37; L[#L+1] = '7.12 '..#t
  do local n = #t; table.insert(t, n > 0 and (1 % (n + 1)) + 1 or 1, 1) end; L[#L+1] = '7.13 '..#t
  do local n = #t; if n > 0 then table.remove(t, (2 % n) + 1) end end; L[#L+1] = '7.14 '..#t
  do local n = #t; table.insert(t, n > 0 and (48 % (n + 1)) + 1 or 1, 7) end; L[#L+1] = '7.15 '..#t
  t[21] = nil; L[#L+1] = '7.16 '..#t
  t[21] = 29; L[#L+1] = '7.17 '..#t
  t[9] = nil; L[#L+1] = '7.18 '..#t
  for _ in ipairs(t) do end; L[#L+1] = '7.19 '..#t
end
do local t = {r0(), nil, 4, nil}; L[#L+1] = '8 c '..#t
  t[27] = 27; L[#L+1] = '8.0 '..#t
  t[9.5] = 1; L[#L+1] = '8.1 '..#t
  t[24] = 86; L[#L+1] = '8.2 '..#t
  t[9] = nil; L[#L+1] = '8.3 '..#t
  do local n = #t; table.insert(t, n > 0 and (23 % (n + 1)) + 1 or 1, 9) end; L[#L+1] = '8.4 '..#t
  t[5.5] = 1; L[#L+1] = '8.5 '..#t
  table.remove(t); L[#L+1] = '8.6 '..#t
  t[#t + 1] = nil; L[#L+1] = '8.7 '..#t
  t[6.5] = 1; L[#L+1] = '8.8 '..#t
  t[40] = 73; L[#L+1] = '8.9 '..#t
  table.remove(t); L[#L+1] = '8.10 '..#t
  do local n = #t; table.insert(t, n > 0 and (12 % (n + 1)) + 1 or 1, 4) end; L[#L+1] = '8.11 '..#t
end
do local t = {}; L[#L+1] = '9 c '..#t
  table.insert(t, 8); L[#L+1] = '9.0 '..#t
  t[#t + 1] = 64; L[#L+1] = '9.1 '..#t
  do local n = #t; if n > 0 then table.remove(t, (7 % n) + 1) end end; L[#L+1] = '9.2 '..#t
  t[7] = nil; L[#L+1] = '9.3 '..#t
  t[13] = nil; L[#L+1] = '9.4 '..#t
  t[3.5] = 1; L[#L+1] = '9.5 '..#t
  do local n = #t; table.insert(t, n > 0 and (20 % (n + 1)) + 1 or 1, 5) end; L[#L+1] = '9.6 '..#t
  t[#t + 1] = 48; L[#L+1] = '9.7 '..#t
  table.insert(t, 6); L[#L+1] = '9.8 '..#t
  t[6] = 73; L[#L+1] = '9.9 '..#t
  t[#t + 1] = 2; L[#L+1] = '9.10 '..#t
  t[14] = nil; L[#L+1] = '9.11 '..#t
  table.insert(t, 8); L[#L+1] = '9.12 '..#t
  table.remove(t); L[#L+1] = '9.13 '..#t
  t[10] = 85; L[#L+1] = '9.14 '..#t
end
do local t = {2, 8, nil, r0(), r0(), r0(), 5, 4, 9, 6, r1(1), 7, nil, r1(6), r1(9), 3, unpack({2, 8, nil, r0(), r0()})}; L[#L+1] = '10 c '..#t
  do local n = #t; if n > 0 then table.remove(t, (1 % n) + 1) end end; L[#L+1] = '10.0 '..#t
  t[4] = 73; L[#L+1] = '10.1 '..#t
  t[2] = 55; L[#L+1] = '10.2 '..#t
  t[35] = 70; L[#L+1] = '10.3 '..#t
  t[6] = 14; L[#L+1] = '10.4 '..#t
  X = t[6]; L[#L+1] = '10.5 '..#t
  t[10] = 84; L[#L+1] = '10.6 '..#t
  t[16] = nil; L[#L+1] = '10.7 '..#t
  do local n = #t; if n > 0 then table.remove(t, (21 % n) + 1) end end; L[#L+1] = '10.8 '..#t
  do local n = #t; if n > 0 then table.remove(t, (22 % n) + 1) end end; L[#L+1] = '10.9 '..#t
end
do local t = {}; L[#L+1] = '13 c '..#t
  for _ in ipairs(t) do end; L[#L+1] = '13.0 '..#t
  do local n = #t; table.insert(t, n > 0 and (33 % (n + 1)) + 1 or 1, 1) end; L[#L+1] = '13.1 '..#t
  t[13] = 84; L[#L+1] = '13.2 '..#t
  t[13] = nil; L[#L+1] = '13.3 '..#t
  t[22] = nil; L[#L+1] = '13.4 '..#t
  t[3.5] = 1; L[#L+1] = '13.5 '..#t
  t[35] = nil; L[#L+1] = '13.6 '..#t
  t[#t + 1] = 66; L[#L+1] = '13.7 '..#t
  t[13] = nil; L[#L+1] = '13.8 '..#t
  t[#t + 1] = 4; L[#L+1] = '13.9 '..#t
  do local n = #t; table.insert(t, n > 0 and (49 % (n + 1)) + 1 or 1, 2) end; L[#L+1] = '13.10 '..#t
  t[31] = 90; L[#L+1] = '13.11 '..#t
  t[6] = nil; L[#L+1] = '13.12 '..#t
  t[4] = nil; L[#L+1] = '13.13 '..#t
  t[#t + 1] = 73; L[#L+1] = '13.14 '..#t
  t[#t + 1] = 34; L[#L+1] = '13.15 '..#t
  t[22] = 16; L[#L+1] = '13.16 '..#t
  X = t[1]; L[#L+1] = '13.17 '..#t
  t[24] = 74; L[#L+1] = '13.18 '..#t
  t[6] = nil; L[#L+1] = '13.19 '..#t
  table.insert(t, 8); L[#L+1] = '13.20 '..#t
  t[12] = nil; L[#L+1] = '13.21 '..#t
  X = t[9]; L[#L+1] = '13.22 '..#t
  t[25] = 82; L[#L+1] = '13.23 '..#t
  t[3.5] = 1; L[#L+1] = '13.24 '..#t
end
do local t = {3, 2, nil, 9, 4, nil, 1, nil, r1(7), r2(1), nil, 7, r0(), r0(), nil, 5, 8, r1(7), r1(2), nil, r0(), 9, 7, r0(), 1, 7, nil, nil, r1(5), r0(), r0(), 8, 7, r0(5)}; L[#L+1] = '14 c '..#t
  table.remove(t); L[#L+1] = '14.0 '..#t
  X = t[35]; L[#L+1] = '14.1 '..#t
  t[26] = nil; L[#L+1] = '14.2 '..#t
  t[9.5] = 1; L[#L+1] = '14.3 '..#t
  t[8] = nil; L[#L+1] = '14.4 '..#t
  do local n = #t; table.insert(t, n > 0 and (36 % (n + 1)) + 1 or 1, 2) end; L[#L+1] = '14.5 '..#t
  X = t[7]; L[#L+1] = '14.6 '..#t
  t[#t + 1] = 69; L[#L+1] = '14.7 '..#t
  table.insert(t, 1); L[#L+1] = '14.8 '..#t
  t[20] = 23; L[#L+1] = '14.9 '..#t
  t[29] = nil; L[#L+1] = '14.10 '..#t
  t[19] = nil; L[#L+1] = '14.11 '..#t
  t[20] = 41; L[#L+1] = '14.12 '..#t
  t[32] = 61; L[#L+1] = '14.13 '..#t
  t[5.5] = 1; L[#L+1] = '14.14 '..#t
  t[4.5] = 1; L[#L+1] = '14.15 '..#t
  t[23] = nil; L[#L+1] = '14.16 '..#t
  t[36] = 81; L[#L+1] = '14.17 '..#t
  t[4] = 40; L[#L+1] = '14.18 '..#t
  table.remove(t); L[#L+1] = '14.19 '..#t
  for _ in ipairs(t) do end; L[#L+1] = '14.20 '..#t
  t[6.5] = 1; L[#L+1] = '14.21 '..#t
  table.remove(t); L[#L+1] = '14.22 '..#t
  t[8] = nil; L[#L+1] = '14.23 '..#t
  t[4.5] = 1; L[#L+1] = '14.24 '..#t
  t[16] = 38; L[#L+1] = '14.25 '..#t
  t[9.5] = 1; L[#L+1] = '14.26 '..#t
  X = t[38]; L[#L+1] = '14.27 '..#t
  t[#t + 1] = nil; L[#L+1] = '14.28 '..#t
  t[15] = 82; L[#L+1] = '14.29 '..#t
  do local n = #t; table.insert(t, n > 0 and (4 % (n + 1)) + 1 or 1, 8) end; L[#L+1] = '14.30 '..#t
  X = t[5]; L[#L+1] = '14.31 '..#t
  t[#t + 1] = 76; L[#L+1] = '14.32 '..#t
  t[17] = 73; L[#L+1] = '14.33 '..#t
  t[19] = nil; L[#L+1] = '14.34 '..#t
end
do local t = {9, 3, r2(1), 8, r0(), nil, 8, 2, r2(5)}; L[#L+1] = '15 c '..#t
  t[31] = nil; L[#L+1] = '15.0 '..#t
  t[#t + 1] = nil; L[#L+1] = '15.1 '..#t
  do local n = #t; table.insert(t, n > 0 and (47 % (n + 1)) + 1 or 1, 9) end; L[#L+1] = '15.2 '..#t
  t[11] = 56; L[#L+1] = '15.3 '..#t
  t[29] = 33; L[#L+1] = '15.4 '..#t
  t[5.5] = 1; L[#L+1] = '15.5 '..#t
  t[#t + 1] = nil; L[#L+1] = '15.6 '..#t
  table.insert(t, 1); L[#L+1] = '15.7 '..#t
  t[4] = 24; L[#L+1] = '15.8 '..#t
  t[39] = 64; L[#L+1] = '15.9 '..#t
  t[8.5] = 1; L[#L+1] = '15.10 '..#t
  table.remove(t); L[#L+1] = '15.11 '..#t
  t[12] = 70; L[#L+1] = '15.12 '..#t
  table.insert(t, 3); L[#L+1] = '15.13 '..#t
  t[28] = 41; L[#L+1] = '15.14 '..#t
  t[5.5] = 1; L[#L+1] = '15.15 '..#t
  do local n = #t; if n > 0 then table.remove(t, (5 % n) + 1) end end; L[#L+1] = '15.16 '..#t
  do local n = #t; table.insert(t, n > 0 and (10 % (n + 1)) + 1 or 1, 7) end; L[#L+1] = '15.17 '..#t
  t[11] = 85; L[#L+1] = '15.18 '..#t
  t[1.5] = 1; L[#L+1] = '15.19 '..#t
  t[30] = 28; L[#L+1] = '15.20 '..#t
  table.remove(t); L[#L+1] = '15.21 '..#t
  t[39] = nil; L[#L+1] = '15.22 '..#t
  t[30] = 6; L[#L+1] = '15.23 '..#t
  t[27] = nil; L[#L+1] = '15.24 '..#t
  t[26] = 61; L[#L+1] = '15.25 '..#t
  t[2] = nil; L[#L+1] = '15.26 '..#t
  do local n = #t; table.insert(t, n > 0 and (14 % (n + 1)) + 1 or 1, 5) end; L[#L+1] = '15.27 '..#t
end
do local t = {[5]=1, 4, nil, [2]=7, [2]=1, 8, [5]=3, [6]=6}; L[#L+1] = '16 c '..#t
  t[12] = nil; L[#L+1] = '16.0 '..#t
  t[4.5] = 1; L[#L+1] = '16.1 '..#t
  t[15] = nil; L[#L+1] = '16.2 '..#t
  do local n = #t; if n > 0 then table.remove(t, (28 % n) + 1) end end; L[#L+1] = '16.3 '..#t
  t[#t + 1] = nil; L[#L+1] = '16.4 '..#t
  t[25] = 7; L[#L+1] = '16.5 '..#t
  table.remove(t); L[#L+1] = '16.6 '..#t
  for _ in ipairs(t) do end; L[#L+1] = '16.7 '..#t
  t[12] = nil; L[#L+1] = '16.8 '..#t
  t[#t + 1] = nil; L[#L+1] = '16.9 '..#t
  table.remove(t); L[#L+1] = '16.10 '..#t
  t[4] = 47; L[#L+1] = '16.11 '..#t
  t[16] = nil; L[#L+1] = '16.12 '..#t
  X = t[2]; L[#L+1] = '16.13 '..#t
  for _ in ipairs(t) do end; L[#L+1] = '16.14 '..#t
  t[11] = nil; L[#L+1] = '16.15 '..#t
  t[34] = 39; L[#L+1] = '16.16 '..#t
  t[16] = nil; L[#L+1] = '16.17 '..#t
  t[10] = 17; L[#L+1] = '16.18 '..#t
  t[20] = 62; L[#L+1] = '16.19 '..#t
  t[#t + 1] = 71; L[#L+1] = '16.20 '..#t
  t[29] = 19; L[#L+1] = '16.21 '..#t
  X = t[18]; L[#L+1] = '16.22 '..#t
  t[#t + 1] = 50; L[#L+1] = '16.23 '..#t
  t[#t + 1] = 38; L[#L+1] = '16.24 '..#t
  t[7] = nil; L[#L+1] = '16.25 '..#t
  t[2] = nil; L[#L+1] = '16.26 '..#t
  t[#t + 1] = nil; L[#L+1] = '16.27 '..#t
  for _ in ipairs(t) do end; L[#L+1] = '16.28 '..#t
  do local n = #t; table.insert(t, n > 0 and (13 % (n + 1)) + 1 or 1, 9) end; L[#L+1] = '16.29 '..#t
  table.remove(t); L[#L+1] = '16.30 '..#t
end
do local t = {r1(5), 7, [6]=7, [11]=4, [9]=2}; L[#L+1] = '17 c '..#t
  X = t[8]; L[#L+1] = '17.0 '..#t
  t[13] = nil; L[#L+1] = '17.1 '..#t
  t[25] = 75; L[#L+1] = '17.2 '..#t
  t[21] = nil; L[#L+1] = '17.3 '..#t
  t[3] = 48; L[#L+1] = '17.4 '..#t
  table.insert(t, 3); L[#L+1] = '17.5 '..#t
end
do local t = {6, r2(1), 4, r1(1), nil, 7, 2, r1(8), 7, 3, nil, 8, 1, 4, 5, nil, 1, r0(), r2(1), 9, 9, 9, r0(), nil, r1(5), 1, 4, nil, nil, r2(1), r1(3), 8, r1(5), 5, 9, 4, 3, r0(), nil, r0(), r2(1), r1(5), 8, 8, 7, r0(), 5, 7, 5, 6, 4, r1(3), nil, 3, nil, r1(3), 7, nil, 3, r2(1), unpack({6, r2(1), 4, r1(1), nil, 7, 2, r1(8), 7, 3, nil, 8, 1, 4, 5, nil, 1, r0(), r2(1), 9, 9, 9, r0(), nil, r1(5), 1, 4, nil, nil, r2(1)})}; L[#L+1] = '18 c '..#t
  t[18] = 81; L[#L+1] = '18.0 '..#t
  do local n = #t; table.insert(t, n > 0 and (43 % (n + 1)) + 1 or 1, 3) end; L[#L+1] = '18.1 '..#t
  t[8] = nil; L[#L+1] = '18.2 '..#t
  t[23] = nil; L[#L+1] = '18.3 '..#t
  for _ in ipairs(t) do end; L[#L+1] = '18.4 '..#t
  t[18] = nil; L[#L+1] = '18.5 '..#t
  t[6.5] = 1; L[#L+1] = '18.6 '..#t
  t[29] = nil; L[#L+1] = '18.7 '..#t
  t[4.5] = 1; L[#L+1] = '18.8 '..#t
  t[14] = nil; L[#L+1] = '18.9 '..#t
  do local n = #t; if n > 0 then table.remove(t, (24 % n) + 1) end end; L[#L+1] = '18.10 '..#t
  do local n = #t; table.insert(t, n > 0 and (34 % (n + 1)) + 1 or 1, 5) end; L[#L+1] = '18.11 '..#t
  t[8.5] = 1; L[#L+1] = '18.12 '..#t
end
do local t = {nil, r0(), r0(), 3, r1(5)}; L[#L+1] = '19 c '..#t
  t[37] = 81; L[#L+1] = '19.0 '..#t
  t[8] = nil; L[#L+1] = '19.1 '..#t
  t[13] = 34; L[#L+1] = '19.2 '..#t
  table.insert(t, 2); L[#L+1] = '19.3 '..#t
  t[20] = 1; L[#L+1] = '19.4 '..#t
  t[31] = 26; L[#L+1] = '19.5 '..#t
end
do local t = pk(6, r1(7), nil, 5, 5, r0(), r2(1), 1, 1, r0(), 2, 1, r1(5), 8, 4, 6, 9, 4, r1(8), 9, r1(2), r0(), 7, 1, 3, 7, nil, 5, r1(6), nil, 5, 6, nil, 4, nil, 3, 4, 2, 6, 7, 7, 7, 9, r1(4), 3, 2, nil, 8, r2(1), 7, r1(3), r1(2), nil, r0(), 3, r1(9), r2(1), 7, r0(), 6); L[#L+1] = '20 c '..#t
  t[9.5] = 1; L[#L+1] = '20.0 '..#t
  for _ in ipairs(t) do end; L[#L+1] = '20.1 '..#t
  t[30] = 60; L[#L+1] = '20.2 '..#t
  t[17] = nil; L[#L+1] = '20.3 '..#t
  t[39] = 6; L[#L+1] = '20.4 '..#t
  t[#t + 1] = 58; L[#L+1] = '20.5 '..#t
  t[2] = nil; L[#L+1] = '20.6 '..#t
  table.insert(t, 3); L[#L+1] = '20.7 '..#t
  t[#t + 1] = nil; L[#L+1] = '20.8 '..#t
  table.remove(t); L[#L+1] = '20.9 '..#t
  t[31] = nil; L[#L+1] = '20.10 '..#t
  t[16] = nil; L[#L+1] = '20.11 '..#t
  t[12] = nil; L[#L+1] = '20.12 '..#t
  do local n = #t; table.insert(t, n > 0 and (7 % (n + 1)) + 1 or 1, 2) end; L[#L+1] = '20.13 '..#t
  t[#t + 1] = 5; L[#L+1] = '20.14 '..#t
  table.remove(t); L[#L+1] = '20.15 '..#t
  t[#t + 1] = 69; L[#L+1] = '20.16 '..#t
  t[28] = 36; L[#L+1] = '20.17 '..#t
  for _ in ipairs(t) do end; L[#L+1] = '20.18 '..#t
  t[36] = 97; L[#L+1] = '20.19 '..#t
  t[#t + 1] = 64; L[#L+1] = '20.20 '..#t
  t[2.5] = 1; L[#L+1] = '20.21 '..#t
  t[6.5] = 1; L[#L+1] = '20.22 '..#t
  t[22] = 45; L[#L+1] = '20.23 '..#t
  for _ in ipairs(t) do end; L[#L+1] = '20.24 '..#t
  t[7.5] = 1; L[#L+1] = '20.25 '..#t
  t[25] = 88; L[#L+1] = '20.26 '..#t
  table.insert(t, 2); L[#L+1] = '20.27 '..#t
  t[14] = 41; L[#L+1] = '20.28 '..#t
  t[9.5] = 1; L[#L+1] = '20.29 '..#t
  do local n = #t; table.insert(t, n > 0 and (24 % (n + 1)) + 1 or 1, 1) end; L[#L+1] = '20.30 '..#t
  t[5.5] = 1; L[#L+1] = '20.31 '..#t
  do local n = #t; table.insert(t, n > 0 and (44 % (n + 1)) + 1 or 1, 4) end; L[#L+1] = '20.32 '..#t
end
do local t = {8, 5, 6, nil, 4, r0(5)}; L[#L+1] = '21 c '..#t
  table.remove(t); L[#L+1] = '21.0 '..#t
  t[4.5] = 1; L[#L+1] = '21.1 '..#t
  t[18] = 79; L[#L+1] = '21.2 '..#t
  t[5] = nil; L[#L+1] = '21.3 '..#t
  t[#t + 1] = 91; L[#L+1] = '21.4 '..#t
  t[19] = nil; L[#L+1] = '21.5 '..#t
  t[8] = nil; L[#L+1] = '21.6 '..#t
  t[6] = 59; L[#L+1] = '21.7 '..#t
  t[31] = 96; L[#L+1] = '21.8 '..#t
end
do local t = pk(2, 6, 8, nil, 1, nil, 7, 5); L[#L+1] = '22 c '..#t
  t[4] = nil; L[#L+1] = '22.0 '..#t
  t[2.5] = 1; L[#L+1] = '22.1 '..#t
  t[#t + 1] = 27; L[#L+1] = '22.2 '..#t
  t[24] = 4; L[#L+1] = '22.3 '..#t
  table.remove(t); L[#L+1] = '22.4 '..#t
  for _ in ipairs(t) do end; L[#L+1] = '22.5 '..#t
  do local n = #t; table.insert(t, n > 0 and (30 % (n + 1)) + 1 or 1, 2) end; L[#L+1] = '22.6 '..#t
  t[17] = nil; L[#L+1] = '22.7 '..#t
  t[39] = 29; L[#L+1] = '22.8 '..#t
  for _ in ipairs(t) do end; L[#L+1] = '22.9 '..#t
  t[18] = nil; L[#L+1] = '22.10 '..#t
  table.remove(t); L[#L+1] = '22.11 '..#t
  t[30] = 84; L[#L+1] = '22.12 '..#t
  X = t[34]; L[#L+1] = '22.13 '..#t
  for _ in ipairs(t) do end; L[#L+1] = '22.14 '..#t
  t[#t + 1] = 13; L[#L+1] = '22.15 '..#t
  do local n = #t; if n > 0 then table.remove(t, (0 % n) + 1) end end; L[#L+1] = '22.16 '..#t
  t[40] = nil; L[#L+1] = '22.17 '..#t
  X = t[31]; L[#L+1] = '22.18 '..#t
  do local n = #t; if n > 0 then table.remove(t, (27 % n) + 1) end end; L[#L+1] = '22.19 '..#t
  t[#t + 1] = 47; L[#L+1] = '22.20 '..#t
  for _ in ipairs(t) do end; L[#L+1] = '22.21 '..#t
  t[27] = 69; L[#L+1] = '22.22 '..#t
  for _ in ipairs(t) do end; L[#L+1] = '22.23 '..#t
  t[33] = 9; L[#L+1] = '22.24 '..#t
  table.insert(t, 2); L[#L+1] = '22.25 '..#t
  t[#t + 1] = 32; L[#L+1] = '22.26 '..#t
  t[27] = 28; L[#L+1] = '22.27 '..#t
  table.insert(t, 9); L[#L+1] = '22.28 '..#t
end
do local t = {2, r1(9), 9, 2, 1, 2, nil, nil, 9, nil, 8, 9, nil, r1(1), 5, 5, 7, 8, 9, r0(), nil, 2, 5, 5, 2, 8, 7, 2, 9, 9, 6, 4, nil, nil, 6, 4, 9, 1, 2, 6, r2(1), 6, r1(4), nil, nil, 8, 6, 2, 5, r1(7), nil}; L[#L+1] = '23 c '..#t
  t[19] = nil; L[#L+1] = '23.0 '..#t
  do local n = #t; table.insert(t, n > 0 and (25 % (n + 1)) + 1 or 1, 4) end; L[#L+1] = '23.1 '..#t
  t[1.5] = 1; L[#L+1] = '23.2 '..#t
  t[10] = nil; L[#L+1] = '23.3 '..#t
  do local n = #t; table.insert(t, n > 0 and (48 % (n + 1)) + 1 or 1, 8) end; L[#L+1] = '23.4 '..#t
  t[40] = 18; L[#L+1] = '23.5 '..#t
  X = t[2]; L[#L+1] = '23.6 '..#t
  table.insert(t, 9); L[#L+1] = '23.7 '..#t
  t[31] = 87; L[#L+1] = '23.8 '..#t
  do local n = #t; table.insert(t, n > 0 and (11 % (n + 1)) + 1 or 1, 2) end; L[#L+1] = '23.9 '..#t
  t[13] = nil; L[#L+1] = '23.10 '..#t
  do local n = #t; if n > 0 then table.remove(t, (7 % n) + 1) end end; L[#L+1] = '23.11 '..#t
  t[4] = nil; L[#L+1] = '23.12 '..#t
  for _ in ipairs(t) do end; L[#L+1] = '23.13 '..#t
  do local n = #t; table.insert(t, n > 0 and (45 % (n + 1)) + 1 or 1, 2) end; L[#L+1] = '23.14 '..#t
  table.remove(t); L[#L+1] = '23.15 '..#t
  t[36] = 28; L[#L+1] = '23.16 '..#t
  t[21] = 16; L[#L+1] = '23.17 '..#t
  t[18] = 5; L[#L+1] = '23.18 '..#t
  X = t[25]; L[#L+1] = '23.19 '..#t
  t[32] = 89; L[#L+1] = '23.20 '..#t
  t[8.5] = 1; L[#L+1] = '23.21 '..#t
  t[11] = nil; L[#L+1] = '23.22 '..#t
  t[1] = 21; L[#L+1] = '23.23 '..#t
  do local n = #t; table.insert(t, n > 0 and (37 % (n + 1)) + 1 or 1, 9) end; L[#L+1] = '23.24 '..#t
  table.remove(t); L[#L+1] = '23.25 '..#t
  table.insert(t, 6); L[#L+1] = '23.26 '..#t
  do local n = #t; if n > 0 then table.remove(t, (30 % n) + 1) end end; L[#L+1] = '23.27 '..#t
end
do local t = {8, 5, 7, nil, 2, r1(5), 4, 2, nil, r2(1), r0(), 8, 3, r1(2), nil, 3, 6, nil, nil, 3, unpack({8, 5, 7, nil, 2, r1(5), 4, 2, nil, r2(1), r0(), 8, 3, r1(2)})}; L[#L+1] = '24 c '..#t
  t[4.5] = 1; L[#L+1] = '24.0 '..#t
  t[4.5] = 1; L[#L+1] = '24.1 '..#t
  t[6.5] = 1; L[#L+1] = '24.2 '..#t
  t[13] = 48; L[#L+1] = '24.3 '..#t
  table.remove(t); L[#L+1] = '24.4 '..#t
  t[19] = 94; L[#L+1] = '24.5 '..#t
  for _ in ipairs(t) do end; L[#L+1] = '24.6 '..#t
  t[2] = 33; L[#L+1] = '24.7 '..#t
  do local n = #t; table.insert(t, n > 0 and (16 % (n + 1)) + 1 or 1, 9) end; L[#L+1] = '24.8 '..#t
  t[32] = 4; L[#L+1] = '24.9 '..#t
  t[18] = nil; L[#L+1] = '24.10 '..#t
  t[25] = 70; L[#L+1] = '24.11 '..#t
  t[2.5] = 1; L[#L+1] = '24.12 '..#t
  t[#t + 1] = 85; L[#L+1] = '24.13 '..#t
  table.remove(t); L[#L+1] = '24.14 '..#t
  do local n = #t; if n > 0 then table.remove(t, (19 % n) + 1) end end; L[#L+1] = '24.15 '..#t
  t[24] = 70; L[#L+1] = '24.16 '..#t
  t[29] = 45; L[#L+1] = '24.17 '..#t
  table.remove(t); L[#L+1] = '24.18 '..#t
  t[23] = nil; L[#L+1] = '24.19 '..#t
  t[36] = 85; L[#L+1] = '24.20 '..#t
  do local n = #t; if n > 0 then table.remove(t, (0 % n) + 1) end end; L[#L+1] = '24.21 '..#t
  t[37] = nil; L[#L+1] = '24.22 '..#t
  t[24] = 19; L[#L+1] = '24.23 '..#t
  t[5] = 53; L[#L+1] = '24.24 '..#t
  table.insert(t, 9); L[#L+1] = '24.25 '..#t
  t[34] = 12; L[#L+1] = '24.26 '..#t
end
do local t = {}; L[#L+1] = '25 c '..#t
  X = t[6]; L[#L+1] = '25.0 '..#t
  do local n = #t; if n > 0 then table.remove(t, (2 % n) + 1) end end; L[#L+1] = '25.1 '..#t
  t[20] = nil; L[#L+1] = '25.2 '..#t
  for _ in ipairs(t) do end; L[#L+1] = '25.3 '..#t
  t[6] = nil; L[#L+1] = '25.4 '..#t
  table.remove(t); L[#L+1] = '25.5 '..#t
  t[1.5] = 1; L[#L+1] = '25.6 '..#t
  t[13] = nil; L[#L+1] = '25.7 '..#t
  t[6.5] = 1; L[#L+1] = '25.8 '..#t
  t[23] = 99; L[#L+1] = '25.9 '..#t
  t[4] = 68; L[#L+1] = '25.10 '..#t
  table.remove(t); L[#L+1] = '25.11 '..#t
  do local n = #t; table.insert(t, n > 0 and (41 % (n + 1)) + 1 or 1, 4) end; L[#L+1] = '25.12 '..#t
  X = t[27]; L[#L+1] = '25.13 '..#t
  t[10] = 58; L[#L+1] = '25.14 '..#t
  t[#t + 1] = 87; L[#L+1] = '25.15 '..#t
  do local n = #t; table.insert(t, n > 0 and (38 % (n + 1)) + 1 or 1, 9) end; L[#L+1] = '25.16 '..#t
  table.insert(t, 1); L[#L+1] = '25.17 '..#t
  t[15] = 66; L[#L+1] = '25.18 '..#t
  t[14] = 32; L[#L+1] = '25.19 '..#t
  t[#t + 1] = 65; L[#L+1] = '25.20 '..#t
  t[14] = 72; L[#L+1] = '25.21 '..#t
  for _ in ipairs(t) do end; L[#L+1] = '25.22 '..#t
  do local n = #t; table.insert(t, n > 0 and (4 % (n + 1)) + 1 or 1, 7) end; L[#L+1] = '25.23 '..#t
  for _ in ipairs(t) do end; L[#L+1] = '25.24 '..#t
end
do local t = {4, 5, nil, r0(), 1, 1, 2, 3, unpack({4, 5})}; L[#L+1] = '26 c '..#t
  t[32] = nil; L[#L+1] = '26.0 '..#t
  X = t[22]; L[#L+1] = '26.1 '..#t
  t[9] = 92; L[#L+1] = '26.2 '..#t
  t[3] = 18; L[#L+1] = '26.3 '..#t
  table.remove(t); L[#L+1] = '26.4 '..#t
  X = t[30]; L[#L+1] = '26.5 '..#t
  t[9] = nil; L[#L+1] = '26.6 '..#t
  t[5] = nil; L[#L+1] = '26.7 '..#t
  table.remove(t); L[#L+1] = '26.8 '..#t
end
do local t = {2, 9, 8, r1(9), 2, 4, 5, r2(1), r0(), nil, 6, nil}; L[#L+1] = '27 c '..#t
  t[39] = 78; L[#L+1] = '27.0 '..#t
  for _ in ipairs(t) do end; L[#L+1] = '27.1 '..#t
  t[#t + 1] = 61; L[#L+1] = '27.2 '..#t
  for _ in ipairs(t) do end; L[#L+1] = '27.3 '..#t
  t[13] = nil; L[#L+1] = '27.4 '..#t
  t[#t + 1] = 23; L[#L+1] = '27.5 '..#t
  t[1] = nil; L[#L+1] = '27.6 '..#t
  t[28] = nil; L[#L+1] = '27.7 '..#t
  t[#t + 1] = 24; L[#L+1] = '27.8 '..#t
  t[11] = 6; L[#L+1] = '27.9 '..#t
  t[40] = 89; L[#L+1] = '27.10 '..#t
end
do local t = pk(nil, 6, 7, 8, 8, 3, 9, r2(1), 3); L[#L+1] = '28 c '..#t
  t[38] = 90; L[#L+1] = '28.0 '..#t
  for _ in ipairs(t) do end; L[#L+1] = '28.1 '..#t
  do local n = #t; table.insert(t, n > 0 and (22 % (n + 1)) + 1 or 1, 8) end; L[#L+1] = '28.2 '..#t
  table.insert(t, 9); L[#L+1] = '28.3 '..#t
  do local n = #t; table.insert(t, n > 0 and (42 % (n + 1)) + 1 or 1, 8) end; L[#L+1] = '28.4 '..#t
  t[19] = nil; L[#L+1] = '28.5 '..#t
  t[8.5] = 1; L[#L+1] = '28.6 '..#t
  t[21] = 62; L[#L+1] = '28.7 '..#t
  t[10] = nil; L[#L+1] = '28.8 '..#t
  t[29] = 36; L[#L+1] = '28.9 '..#t
  t[7.5] = 1; L[#L+1] = '28.10 '..#t
  t[4.5] = 1; L[#L+1] = '28.11 '..#t
  X = t[33]; L[#L+1] = '28.12 '..#t
  t[23] = 10; L[#L+1] = '28.13 '..#t
  t[#t + 1] = 41; L[#L+1] = '28.14 '..#t
  t[9.5] = 1; L[#L+1] = '28.15 '..#t
  t[13] = 89; L[#L+1] = '28.16 '..#t
  t[3.5] = 1; L[#L+1] = '28.17 '..#t
  table.remove(t); L[#L+1] = '28.18 '..#t
  t[2.5] = 1; L[#L+1] = '28.19 '..#t
  t[4.5] = 1; L[#L+1] = '28.20 '..#t
  t[2] = nil; L[#L+1] = '28.21 '..#t
  do local n = #t; table.insert(t, n > 0 and (50 % (n + 1)) + 1 or 1, 7) end; L[#L+1] = '28.22 '..#t
  X = t[5]; L[#L+1] = '28.23 '..#t
  for _ in ipairs(t) do end; L[#L+1] = '28.24 '..#t
end
do local t = {r2(1), r0(), 3, nil, r1(5), r2(1), nil, 1, 7}; L[#L+1] = '29 c '..#t
  t[5.5] = 1; L[#L+1] = '29.0 '..#t
  for _ in ipairs(t) do end; L[#L+1] = '29.1 '..#t
  t[23] = 21; L[#L+1] = '29.2 '..#t
  t[#t + 1] = 80; L[#L+1] = '29.3 '..#t
  do local n = #t; if n > 0 then table.remove(t, (33 % n) + 1) end end; L[#L+1] = '29.4 '..#t
  t[9] = nil; L[#L+1] = '29.5 '..#t
  table.remove(t); L[#L+1] = '29.6 '..#t
  do local n = #t; table.insert(t, n > 0 and (21 % (n + 1)) + 1 or 1, 6) end; L[#L+1] = '29.7 '..#t
  X = t[2]; L[#L+1] = '29.8 '..#t
  t[38] = 60; L[#L+1] = '29.9 '..#t
  do local n = #t; if n > 0 then table.remove(t, (6 % n) + 1) end end; L[#L+1] = '29.10 '..#t
  t[34] = nil; L[#L+1] = '29.11 '..#t
  t[7] = nil; L[#L+1] = '29.12 '..#t
  t[#t + 1] = nil; L[#L+1] = '29.13 '..#t
  t[#t + 1] = 75; L[#L+1] = '29.14 '..#t
  t[11] = nil; L[#L+1] = '29.15 '..#t
  t[2] = nil; L[#L+1] = '29.16 '..#t
  for _ in ipairs(t) do end; L[#L+1] = '29.17 '..#t
  t[#t + 1] = 53; L[#L+1] = '29.18 '..#t
  X = t[31]; L[#L+1] = '29.19 '..#t
  X = t[6]; L[#L+1] = '29.20 '..#t
  t[28] = 54; L[#L+1] = '29.21 '..#t
  do local n = #t; if n > 0 then table.remove(t, (41 % n) + 1) end end; L[#L+1] = '29.22 '..#t
  X = t[33]; L[#L+1] = '29.23 '..#t
  X = t[17]; L[#L+1] = '29.24 '..#t
  table.insert(t, 1); L[#L+1] = '29.25 '..#t
  t[#t + 1] = 8; L[#L+1] = '29.26 '..#t
  table.insert(t, 9); L[#L+1] = '29.27 '..#t
  for _ in ipairs(t) do end; L[#L+1] = '29.28 '..#t
  table.remove(t); L[#L+1] = '29.29 '..#t
end
do local t = {[4]=4, nil}; L[#L+1] = '30 c '..#t
  t[18] = 79; L[#L+1] = '30.0 '..#t
  t[14] = nil; L[#L+1] = '30.1 '..#t
  t[#t + 1] = 84; L[#L+1] = '30.2 '..#t
  t[40] = 12; L[#L+1] = '30.3 '..#t
  table.remove(t); L[#L+1] = '30.4 '..#t
  t[37] = 3; L[#L+1] = '30.5 '..#t
  t[15] = nil; L[#L+1] = '30.6 '..#t
  table.insert(t, 4); L[#L+1] = '30.7 '..#t
  t[6.5] = 1; L[#L+1] = '30.8 '..#t
  X = t[8]; L[#L+1] = '30.9 '..#t
  t[2] = 93; L[#L+1] = '30.10 '..#t
  t[#t + 1] = nil; L[#L+1] = '30.11 '..#t
  t[5.5] = 1; L[#L+1] = '30.12 '..#t
  t[6] = nil; L[#L+1] = '30.13 '..#t
  t[14] = nil; L[#L+1] = '30.14 '..#t
  t[28] = 80; L[#L+1] = '30.15 '..#t
  t[20] = 12; L[#L+1] = '30.16 '..#t
  t[#t + 1] = 71; L[#L+1] = '30.17 '..#t
  t[#t + 1] = 65; L[#L+1] = '30.18 '..#t
  for _ in ipairs(t) do end; L[#L+1] = '30.19 '..#t
  X = t[5]; L[#L+1] = '30.20 '..#t
  do local n = #t; if n > 0 then table.remove(t, (14 % n) + 1) end end; L[#L+1] = '30.21 '..#t
  table.remove(t); L[#L+1] = '30.22 '..#t
  do local n = #t; table.insert(t, n > 0 and (27 % (n + 1)) + 1 or 1, 4) end; L[#L+1] = '30.23 '..#t
  do local n = #t; table.insert(t, n > 0 and (38 % (n + 1)) + 1 or 1, 2) end; L[#L+1] = '30.24 '..#t
  t[#t + 1] = 51; L[#L+1] = '30.25 '..#t
  t[27] = 55; L[#L+1] = '30.26 '..#t
  t[40] = 51; L[#L+1] = '30.27 '..#t
  t[13] = nil; L[#L+1] = '30.28 '..#t
  t[19] = nil; L[#L+1] = '30.29 '..#t
  t[14] = nil; L[#L+1] = '30.30 '..#t
  t[11] = nil; L[#L+1] = '30.31 '..#t
  do local n = #t; if n > 0 then table.remove(t, (30 % n) + 1) end end; L[#L+1] = '30.32 '..#t
  t[36] = 21; L[#L+1] = '30.33 '..#t
  for _ in ipairs(t) do end; L[#L+1] = '30.34 '..#t
  do local n = #t; if n > 0 then table.remove(t, (20 % n) + 1) end end; L[#L+1] = '30.35 '..#t
  t[1.5] = 1; L[#L+1] = '30.36 '..#t
  t[37] = 64; L[#L+1] = '30.37 '..#t
  t[32] = 38; L[#L+1] = '30.38 '..#t
  X = t[27]; L[#L+1] = '30.39 '..#t
end
do local t = {1, 9, nil, 1, r2(1), unpack({1, 9})}; L[#L+1] = '31 c '..#t
  do local n = #t; if n > 0 then table.remove(t, (0 % n) + 1) end end; L[#L+1] = '31.0 '..#t
  t[11] = 28; L[#L+1] = '31.1 '..#t
  t[5] = nil; L[#L+1] = '31.2 '..#t
  t[17] = nil; L[#L+1] = '31.3 '..#t
  t[24] = 52; L[#L+1] = '31.4 '..#t
  t[20] = 79; L[#L+1] = '31.5 '..#t
  t[12] = 11; L[#L+1] = '31.6 '..#t
  do local n = #t; table.insert(t, n > 0 and (47 % (n + 1)) + 1 or 1, 4) end; L[#L+1] = '31.7 '..#t
  t[32] = 75; L[#L+1] = '31.8 '..#t
  t[19] = nil; L[#L+1] = '31.9 '..#t
  t[24] = 44; L[#L+1] = '31.10 '..#t
  t[8.5] = 1; L[#L+1] = '31.11 '..#t
  table.remove(t); L[#L+1] = '31.12 '..#t
  t[10] = nil; L[#L+1] = '31.13 '..#t
  table.insert(t, 4); L[#L+1] = '31.14 '..#t
  t[#t + 1] = 20; L[#L+1] = '31.15 '..#t
  do local n = #t; table.insert(t, n > 0 and (50 % (n + 1)) + 1 or 1, 8) end; L[#L+1] = '31.16 '..#t
  X = t[12]; L[#L+1] = '31.17 '..#t
  t[3.5] = 1; L[#L+1] = '31.18 '..#t
  t[#t + 1] = nil; L[#L+1] = '31.19 '..#t
  t[21] = nil; L[#L+1] = '31.20 '..#t
  t[24] = 64; L[#L+1] = '31.21 '..#t
  table.remove(t); L[#L+1] = '31.22 '..#t
  t[9.5] = 1; L[#L+1] = '31.23 '..#t
  for _ in ipairs(t) do end; L[#L+1] = '31.24 '..#t
  t[3] = 98; L[#L+1] = '31.25 '..#t
  for _ in ipairs(t) do end; L[#L+1] = '31.26 '..#t
  t[#t + 1] = 14; L[#L+1] = '31.27 '..#t
  t[6] = nil; L[#L+1] = '31.28 '..#t
  table.insert(t, 3); L[#L+1] = '31.29 '..#t
  table.remove(t); L[#L+1] = '31.30 '..#t
  t[19] = 3; L[#L+1] = '31.31 '..#t
end
do local t = {2, 4, r0(), [10]=8, 9, 3, 1, [5]=9, 2, 5}; L[#L+1] = '32 c '..#t
  t[20] = 83; L[#L+1] = '32.0 '..#t
  t[8.5] = 1; L[#L+1] = '32.1 '..#t
  t[13] = 72; L[#L+1] = '32.2 '..#t
  table.remove(t); L[#L+1] = '32.3 '..#t
  do local n = #t; if n > 0 then table.remove(t, (5 % n) + 1) end end; L[#L+1] = '32.4 '..#t
  t[8] = nil; L[#L+1] = '32.5 '..#t
  X = t[13]; L[#L+1] = '32.6 '..#t
  t[16] = 43; L[#L+1] = '32.7 '..#t
  t[#t + 1] = 5; L[#L+1] = '32.8 '..#t
  t[6.5] = 1; L[#L+1] = '32.9 '..#t
  for _ in ipairs(t) do end; L[#L+1] = '32.10 '..#t
  t[#t + 1] = 19; L[#L+1] = '32.11 '..#t
  t[8.5] = 1; L[#L+1] = '32.12 '..#t
  table.insert(t, 9); L[#L+1] = '32.13 '..#t
  table.insert(t, 3); L[#L+1] = '32.14 '..#t
  t[16] = 28; L[#L+1] = '32.15 '..#t
  X = t[35]; L[#L+1] = '32.16 '..#t
  t[1.5] = 1; L[#L+1] = '32.17 '..#t
  t[14] = 32; L[#L+1] = '32.18 '..#t
  do local n = #t; if n > 0 then table.remove(t, (11 % n) + 1) end end; L[#L+1] = '32.19 '..#t
  for _ in ipairs(t) do end; L[#L+1] = '32.20 '..#t
  t[#t + 1] = 93; L[#L+1] = '32.21 '..#t
  t[11] = 5; L[#L+1] = '32.22 '..#t
end
do local t = {r1(1), nil, r0(), nil, r2(1), unpack({r1(1), nil})}; L[#L+1] = '33 c '..#t
  t[8] = 88; L[#L+1] = '33.0 '..#t
  t[37] = 85; L[#L+1] = '33.1 '..#t
  t[12] = 84; L[#L+1] = '33.2 '..#t
  for _ in ipairs(t) do end; L[#L+1] = '33.3 '..#t
  table.insert(t, 4); L[#L+1] = '33.4 '..#t
  do local n = #t; table.insert(t, n > 0 and (16 % (n + 1)) + 1 or 1, 3) end; L[#L+1] = '33.5 '..#t
  table.remove(t); L[#L+1] = '33.6 '..#t
  t[8] = nil; L[#L+1] = '33.7 '..#t
  do local n = #t; table.insert(t, n > 0 and (45 % (n + 1)) + 1 or 1, 6) end; L[#L+1] = '33.8 '..#t
  t[#t + 1] = 76; L[#L+1] = '33.9 '..#t
  t[9] = nil; L[#L+1] = '33.10 '..#t
  t[2] = 78; L[#L+1] = '33.11 '..#t
  t[13] = nil; L[#L+1] = '33.12 '..#t
  t[38] = 78; L[#L+1] = '33.13 '..#t
  t[33] = 85; L[#L+1] = '33.14 '..#t
  do local n = #t; if n > 0 then table.remove(t, (40 % n) + 1) end end; L[#L+1] = '33.15 '..#t
end
do local t = {1, 1, 7, 7, nil, nil, r1(1), 2, 3, nil, r0(), 7, 1, 6, nil, r0(), 2, 3, 2, 2, nil, 3, r1(7), 7, nil, 7, 5, 1, 6, r0(), 7, 5, r0(), 7, r1(6), r0(), 1, 5, 2, nil, r0(), 7, r0(), 5, 2, nil, r0(), 5, r2(1), r1(5), r2(1), r0(5)}; L[#L+1] = '34 c '..#t
  t[3] = nil; L[#L+1] = '34.0 '..#t
  t[4] = 75; L[#L+1] = '34.1 '..#t
  t[21] = nil; L[#L+1] = '34.2 '..#t
  t[15] = nil; L[#L+1] = '34.3 '..#t
  do local n = #t; table.insert(t, n > 0 and (16 % (n + 1)) + 1 or 1, 3) end; L[#L+1] = '34.4 '..#t
  t[7] = nil; L[#L+1] = '34.5 '..#t
  t[#t + 1] = 72; L[#L+1] = '34.6 '..#t
  table.insert(t, 2); L[#L+1] = '34.7 '..#t
  t[1] = 83; L[#L+1] = '34.8 '..#t
  t[33] = 9; L[#L+1] = '34.9 '..#t
  t[16] = nil; L[#L+1] = '34.10 '..#t
  t[30] = nil; L[#L+1] = '34.11 '..#t
  X = t[27]; L[#L+1] = '34.12 '..#t
  t[36] = 41; L[#L+1] = '34.13 '..#t
  table.remove(t); L[#L+1] = '34.14 '..#t
  t[#t + 1] = 84; L[#L+1] = '34.15 '..#t
  for _ in ipairs(t) do end; L[#L+1] = '34.16 '..#t
  t[39] = 5; L[#L+1] = '34.17 '..#t
  t[18] = nil; L[#L+1] = '34.18 '..#t
  table.remove(t); L[#L+1] = '34.19 '..#t
  do local n = #t; table.insert(t, n > 0 and (41 % (n + 1)) + 1 or 1, 1) end; L[#L+1] = '34.20 '..#t
end
do local t = {[1]=1, nil, r0(), 5}; L[#L+1] = '35 c '..#t
  t[9.5] = 1; L[#L+1] = '35.0 '..#t
  t[6] = 45; L[#L+1] = '35.1 '..#t
  X = t[24]; L[#L+1] = '35.2 '..#t
  do local n = #t; table.insert(t, n > 0 and (27 % (n + 1)) + 1 or 1, 9) end; L[#L+1] = '35.3 '..#t
  do local n = #t; table.insert(t, n > 0 and (3 % (n + 1)) + 1 or 1, 9) end; L[#L+1] = '35.4 '..#t
  for _ in ipairs(t) do end; L[#L+1] = '35.5 '..#t
  t[14] = nil; L[#L+1] = '35.6 '..#t
  t[#t + 1] = 34; L[#L+1] = '35.7 '..#t
  t[#t + 1] = 35; L[#L+1] = '35.8 '..#t
  t[9] = 47; L[#L+1] = '35.9 '..#t
  table.remove(t); L[#L+1] = '35.10 '..#t
  t[39] = 36; L[#L+1] = '35.11 '..#t
  t[13] = nil; L[#L+1] = '35.12 '..#t
  table.remove(t); L[#L+1] = '35.13 '..#t
  t[8] = 52; L[#L+1] = '35.14 '..#t
end
do local t = {}; L[#L+1] = '36 c '..#t
  table.insert(t, 8); L[#L+1] = '36.0 '..#t
  do local n = #t; table.insert(t, n > 0 and (23 % (n + 1)) + 1 or 1, 7) end; L[#L+1] = '36.1 '..#t
  X = t[14]; L[#L+1] = '36.2 '..#t
  table.remove(t); L[#L+1] = '36.3 '..#t
  X = t[31]; L[#L+1] = '36.4 '..#t
  t[11] = 96; L[#L+1] = '36.5 '..#t
  table.remove(t); L[#L+1] = '36.6 '..#t
  do local n = #t; table.insert(t, n > 0 and (21 % (n + 1)) + 1 or 1, 8) end; L[#L+1] = '36.7 '..#t
  t[4] = 7; L[#L+1] = '36.8 '..#t
  t[10] = 10; L[#L+1] = '36.9 '..#t
  t[3] = nil; L[#L+1] = '36.10 '..#t
end
do local t = {4, r1(3), r2(1), 4, [14]=6, r1(4), r0(), 2, r1(8), 2, 3, r0(), r0(), 7, r0(), 3, r1(1), 9, 4, r0(), nil, nil, nil, 7, 2, r1(8), 6, 5, 9, 7, 4, 9}; L[#L+1] = '37 c '..#t
  t[7] = 23; L[#L+1] = '37.0 '..#t
  t[10] = nil; L[#L+1] = '37.1 '..#t
  for _ in ipairs(t) do end; L[#L+1] = '37.2 '..#t
  t[19] = nil; L[#L+1] = '37.3 '..#t
  do local n = #t; if n > 0 then table.remove(t, (30 % n) + 1) end end; L[#L+1] = '37.4 '..#t
  t[21] = 21; L[#L+1] = '37.5 '..#t
  t[#t + 1] = nil; L[#L+1] = '37.6 '..#t
  do local n = #t; if n > 0 then table.remove(t, (36 % n) + 1) end end; L[#L+1] = '37.7 '..#t
  t[2.5] = 1; L[#L+1] = '37.8 '..#t
  t[3.5] = 1; L[#L+1] = '37.9 '..#t
end
do local t = {[7]=6, nil, 5, 8}; L[#L+1] = '38 c '..#t
  X = t[14]; L[#L+1] = '38.0 '..#t
  t[18] = 86; L[#L+1] = '38.1 '..#t
  table.insert(t, 7); L[#L+1] = '38.2 '..#t
  t[14] = 6; L[#L+1] = '38.3 '..#t
  t[30] = 18; L[#L+1] = '38.4 '..#t
  table.insert(t, 9); L[#L+1] = '38.5 '..#t
  t[#t + 1] = 28; L[#L+1] = '38.6 '..#t
  t[3.5] = 1; L[#L+1] = '38.7 '..#t
  t[#t + 1] = 97; L[#L+1] = '38.8 '..#t
  t[4.5] = 1; L[#L+1] = '38.9 '..#t
end
do local t = {7, 2, r0(), 3, unpack({7, 2})}; L[#L+1] = '39 c '..#t
  t[35] = nil; L[#L+1] = '39.0 '..#t
  t[1] = nil; L[#L+1] = '39.1 '..#t
  t[#t + 1] = nil; L[#L+1] = '39.2 '..#t
  t[#t + 1] = nil; L[#L+1] = '39.3 '..#t
  table.remove(t); L[#L+1] = '39.4 '..#t
  t[25] = 5; L[#L+1] = '39.5 '..#t
  table.insert(t, 4); L[#L+1] = '39.6 '..#t
  t[2.5] = 1; L[#L+1] = '39.7 '..#t
  t[24] = 18; L[#L+1] = '39.8 '..#t
  t[40] = 23; L[#L+1] = '39.9 '..#t
end
print(table.concat(L, '\n'))
