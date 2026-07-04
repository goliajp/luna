-- v2.14 CV.3: raw* bypass every metamethod.
local t = setmetatable({ real = 1 }, {
  __index = function() return "trap" end,
  __newindex = function() error("trap") end,
  __len = function() return 99 end,
})
print(rawget(t, "real"), rawget(t, "fake"))
rawset(t, "added", 2)
print(t.added ~= "trap" and rawget(t, "added"))
print(rawlen(t), rawlen("hello"))
print(rawequal(t, t), rawequal({}, {}))
