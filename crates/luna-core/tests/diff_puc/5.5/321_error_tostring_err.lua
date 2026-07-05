-- v2.14 CV.2: an error object WITH __tostring reaches the top
-- rendered by it — no position prefix.
error(setmetatable({}, { __tostring = function() return "custom obj err" end }))
