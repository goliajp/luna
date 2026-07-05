-- v2.12 CORPUS-III: nested function upvalue capture.
local function outer()
  local x = 10
  local function inner()
    return x * 2
  end
  x = 20   -- inner sees the update
  return inner()
end
print(outer())   -- 40
