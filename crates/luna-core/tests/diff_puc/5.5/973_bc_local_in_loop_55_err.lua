-- v3.1 bytecode: an error inside a numeric-for body names the body's local.
for i = 1, 2 do
  local s
  s.x = i
end
