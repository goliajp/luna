-- v2.11 CORPUS-II: load+dump round-trip.
local orig = load("return function(x) return x * 3 + 1 end")
if orig then
  local f = orig()
  print(f(4))
end
