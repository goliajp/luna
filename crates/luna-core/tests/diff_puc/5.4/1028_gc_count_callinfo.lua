-- call records made by deep calls (through C functions too) stay with
-- the thread until a collection frees what is not in use
collectgarbage("stop")
if _VERSION >= "Lua 5.4" then collectgarbage("incremental") end
-- a full collection first: the run starts with no cycle in progress
collectgarbage(); collectgarbage("stop")
local function count()
  collectgarbage(); collectgarbage(); collectgarbage("stop")
  return string.format("%.0f", collectgarbage("count") * 1024)
end
local base = count()
local function viapcall(n) if n == 0 then return count() end return select(2, pcall(viapcall, n - 1)) end
print(viapcall(150) - base, count() - base)
local function plain(n) if n == 0 then return 0 end return 1 + plain(n - 1) end
plain(500)
print(count() - base)
