-- A chunk whose main function's code count is N, cut after the count.
local v = _VERSION:sub(5)
local d = string.dump(function() end)
local st = d:byte(9) -- 5.1/5.2: sizeof(size_t)
local function int(x) local t = {} for i = 1, 4 do t[i] = string.char(x % 256) x = math.floor(x / 256) end return table.concat(t) end
local function varint(x, last_high)
  local t = {}
  repeat t[#t + 1] = x % 128 x = math.floor(x / 128) until x == 0
  local out = {}
  for i = #t, 1, -1 do
    local b = t[i]
    if last_high and i == 1 then b = b + 128 end
    if not last_high and i > 1 then b = b + 128 end
    out[#out + 1] = string.char(b)
  end
  return table.concat(out)
end
local n = N
local bad
if v == "5.1" then bad = d:sub(1, 12) .. ("\0"):rep(st) .. int(0) .. int(0) .. "\0\0\2\2" .. int(n) .. "abc"
elseif v == "5.2" then bad = d:sub(1, 18) .. int(0) .. int(0) .. "\0\2\2" .. int(n) .. "abc"
elseif v == "5.3" then bad = d:sub(1, 33) .. "\1" .. "\0" .. int(0) .. int(0) .. "\0\2\2" .. int(n) .. "abc"
elseif v == "5.4" then bad = d:sub(1, 31) .. "\1" .. "\x80" .. "\x80\x80" .. "\0\2\2" .. varint(n, true) .. "abc"
else bad = d:sub(1, 40) .. "\1" .. "\0\0" .. "\0\2\2" .. varint(n, false) .. "abc" end
local ld = loadstring or load
local p = {pcall(function() return ld(bad, "=bad") end)}
local x = {xpcall(function() return ld(bad, "=bad") end, function(m) return "H(" .. tostring(m) .. ")" end)}
return tostring(p[1]) .. " " .. tostring(p[2]) .. " " .. tostring(p[3]) .. "|" .. tostring(x[1]) .. " " .. tostring(x[2]) .. " " .. tostring(x[3])
