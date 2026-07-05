-- v2.10 CORPUS: __call + __len.
local callable = setmetatable({}, {__call = function(self, a, b) return a + b end})
print(callable(3, 4))
print(callable(10, 20))

local sized = setmetatable({}, {__len = function() return 42 end})
print(#sized)
