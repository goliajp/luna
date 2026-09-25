-- v3.1 debug slice: inside a coroutine the natives of the resuming thread
-- are not levels of it, so a function called from Lua there is named by
-- its caller, and debug.getinfo stops at the coroutine's body.
local co = coroutine.wrap(function()
  local _, msg = pcall(function() local x = debug.getlocal() return x end)
  print((msg:gsub("^[^:]+:%d+: ", "")))
  local l, out = 0, {}
  while debug.getinfo(l, "S") do
    out[#out + 1] = debug.getinfo(l, "S").what
    l = l + 1
  end
  print(table.concat(out, ","))
end)
co()
