-- v2.14 CV.2: catching then rethrowing with level 0 keeps the
-- original text untouched.
local ok, e = pcall(function() error("inner boom") end)
error(e, 0)
