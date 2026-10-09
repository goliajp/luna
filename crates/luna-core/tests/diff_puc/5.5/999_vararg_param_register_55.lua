-- 5.5 gives a function's `...` a parameter register after the fixed ones,
-- holding nil: debug.getlocal sees it as "(vararg table)", and a call made
-- from such a function starts one slot higher than from a fixed one (the
-- depth a recursion reaches shows it, printed against the fixed one's)
local function locals(level)
  local out, n = {}, 1
  while true do
    local name, v = debug.getlocal(level + 1, n)
    if not name then break end
    out[#out + 1] = name .. "=" .. (type(v) == "function" and "f" or tostring(v))
    n = n + 1
  end
  return table.concat(out, " ")
end
local function va(a, b, ...) local c = 3 local s = locals(1) return s end
print(va(1, 2, 9, 8))
print(va(1))
local function va0(...) local x = ... local s = locals(1) return s end
print(va0(7))
local function set(...) debug.setlocal(1, 1, "s") return (debug.getlocal(1, 1)) end
print(set(1), select("#", set()))
local depth = 0
local function f() depth = depth + 1 return f() + 1 end
local function fixed(g) depth = 0 pcall(g) return depth end
local function vararg(g, ...) depth = 0 pcall(g) return depth end
local base = fixed(f)
print(vararg(f) - base, vararg(f, 1, 2) - base)
