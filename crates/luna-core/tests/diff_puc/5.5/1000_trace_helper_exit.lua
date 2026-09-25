-- v3.1 trace JIT: a table operation the compiled code leaves to the
-- interpreter (the table has a metatable) part-way through a loop exits
-- the trace at that op; what the loop already did does not run again.

-- a string-key store
local ts = {}
for i = 1, 300 do ts[i] = {} end
setmetatable(ts[250], {})
local function run(total, ts)
  for i = 1, 300 do
    total.n = total.n + 1
    local x = ts[i]
    x.w = i
  end
end
local total = {n = 0}
run(total, ts)
print(total.n, ts[300].w, ts[250].w)

-- an integer-key store that __newindex sees
local seen = 0
local arr = {}
for i = 1, 300 do arr[i] = {} end
setmetatable(arr[200], {__newindex = function(t, k, v) seen = seen + 1; rawset(t, k, v) end})
local function fill(count, arr)
  for i = 1, 300 do
    count[1] = count[1] + 1
    arr[i][1] = i
  end
end
local count = {0}
fill(count, arr)
print(count[1], arr[300][1], arr[200][1], seen)

-- a length that __len answers
local lens = {}
for i = 1, 300 do lens[i] = {1, 2, 3} end
setmetatable(lens[150], {__len = function() return 100 end})
local function sum_len(acc, lens)
  for i = 1, 300 do
    acc.calls = acc.calls + 1
    acc.sum = acc.sum + #lens[i]
  end
end
local acc = {calls = 0, sum = 0}
sum_len(acc, lens)
print(acc.calls, acc.sum)

-- a concatenation that __concat answers
local parts = {}
for i = 1, 300 do parts[i] = "p" end
parts[180] = setmetatable({}, {__concat = function() return "M" end})
local function join(st, parts)
  for i = 1, 300 do
    st.k = st.k + 1
    st.s = st.s .. parts[i]
  end
end
local st = {k = 0, s = ""}
join(st, parts)
print(st.k, #st.s, st.s:sub(1, 3))
