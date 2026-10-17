-- the order next gives over each table the libraries make (5.1 has no
-- hash seed: it is the same on every run); `arg` and the global a test
-- harness may add are left out
local seen, list = {}, {}
local function name(n, t)
  if type(t) ~= "table" or seen[t] then return end
  seen[t] = n
  list[#list + 1] = {n, t}
end
local reg = debug.getregistry()
name("_G", _G)
name("registry", reg)
name("string_mt", getmetatable(""))
name("FILE*", getmetatable(io.stdout))
local function sub(prefix, t)
  local keys = {}
  for k, v in pairs(t) do
    if type(v) == "table" and type(k) == "string" then keys[#keys + 1] = k end
  end
  table.sort(keys)
  for _, k in ipairs(keys) do name(prefix .. k, t[k]) end
end
sub("", _G)
sub("registry.", reg)
sub("package.", package)
table.sort(list, function(a, b) return a[1] < b[1] end)
for _, e in ipairs(list) do
  if e[1] ~= "arg" then
    local keys = {}
    for k in next, e[2] do
      if k ~= "arg" and k ~= "__luna_diff_puc_buf" then keys[#keys + 1] = tostring(k) end
    end
    print(e[1] .. ": " .. table.concat(keys, " "))
  end
end
