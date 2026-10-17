-- the libraries' tables a name reaches, with the keys `next` gives, in
-- its order (returned as text; .dev's pairsorder.lua prints the same):
--   T <name>, then one key per line (a string as it is, a number as #n,
--   anything else as ?type)
local seen, list, out = {}, {}, {}
local function name(n, t)
  if type(t) ~= "table" or seen[t] then return end
  seen[t] = n
  list[#list + 1] = {n, t}
end
local reg = debug.getregistry()
name("_G", _G)
name("registry", reg)
name("string_mt", getmetatable(""))
if io and io.stdout then name("FILE*", getmetatable(io.stdout)) end
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
if package then
  sub("package.", package)
  if package.loaded then sub("package.loaded.", package.loaded) end
end
local fmt = getmetatable(io and io.stdout or 0)
if type(fmt) == "table" then sub("FILE*.", fmt) end
table.sort(list, function(a, b) return a[1] < b[1] end)
for _, e in ipairs(list) do
  out[#out + 1] = "T " .. e[1] .. "\n"
  for k in next, e[2] do
    local s = type(k) == "string" and k or type(k) == "number" and ("#" .. k) or ("?" .. type(k))
    out[#out + 1] = s .. "\n"
  end
end
return table.concat(out)
