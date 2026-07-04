-- v2.14 HD 5.4 seed: the generic-for control variable is a
-- plain local in 5.4 — assignment compiles and does not affect
-- iteration (contrast: <const> compile error in 5.5).
local out = {}
for i in pairs({ 10 }) do
  local captured = i
  i = 999
  out[#out + 1] = captured
end
print(#out, out[1])
local f = load("for i in pairs({}) do i = 1 end")
print(f ~= nil)
