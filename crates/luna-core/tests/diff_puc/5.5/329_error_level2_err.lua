-- v2.14 CV.2: error(msg, 2) blames the CALLER's line.
local function raiser()
  error("blamed on caller", 2)
end
raiser()
