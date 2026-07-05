-- v2.15 P2.4 utf8: len returns nil + position on invalid seq.
local s = "hi" .. string.char(0xff) .. "!"    -- 0xff is invalid start byte
local n, pos = utf8.len(s)
print(n, pos)
