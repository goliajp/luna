-- v2.14 CV.2: an error raised inside __index propagates with its
-- own position info intact.
local t = setmetatable({}, { __index = function() error("from_index", 0) end })
return t.anything
