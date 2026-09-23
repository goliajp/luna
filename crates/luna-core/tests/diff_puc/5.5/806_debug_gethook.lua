-- v3.1 debug slice: debug.gethook's results per version, including a hook
-- set with an empty mask (which lua_sethook drops).
local function hook() end
print(select("#", debug.gethook()), debug.gethook())
debug.sethook(hook, "")
print(select("#", debug.gethook()), debug.gethook() == hook)
debug.sethook(hook, "", 3)
print(select("#", debug.gethook()), debug.gethook() == hook, select(2, debug.gethook()))
debug.sethook()
print(pcall(debug.sethook, hook, 1))
print(pcall(debug.sethook, hook, {}))
print(pcall(debug.sethook, 1, "c"))
print(pcall(debug.sethook, hook))
