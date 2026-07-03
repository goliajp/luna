-- v2.13 CORPUS-IV: locals shadow globals of the same name;
-- global writes via _G while shadowed.
print = print   -- global self-assign is a no-op
local tostring = function(v) return "local_ts" end
print(tostring(42))
print(_G.tostring(42))
do
  local print = function(...) _G.print("wrapped:", ...) end
  print("inner")
end
print("outer")
_G.shadow_probe = "set_via_G"
local shadow_probe = "local_ver"
print(shadow_probe, _G.shadow_probe)
_G.shadow_probe = nil
