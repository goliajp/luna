-- a truncated binary chunk fails inside luaD_protectedparser, whose
-- luaD_pcall catches the error and shrinks the stack
collectgarbage("stop")
local function deep(n) if n > 0 then return deep(n - 1) + 1 end return 0 end
local function count() return string.format("%.0f", collectgarbage("count") * 1024) end
local chunk = string.dump(function() return 1 end):sub(1, 20)
deep(300)
local base = count()
local f, e = load(chunk)
local c = count() - base
print(f, (e:gsub("^.-:%d+: ", "")), c)
