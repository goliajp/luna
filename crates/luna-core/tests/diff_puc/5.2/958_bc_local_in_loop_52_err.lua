-- v3.1 bytecode: an error inside a generic-for body names the body's local.
for k, v in pairs({a = 1}) do
  local z
  z.f = k
end
