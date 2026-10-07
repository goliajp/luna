-- Non-ASCII file names and environment values as the C library's narrow
-- functions take them. File names are under the global DIR when it is set;
-- the messages print the names as given.
local D = DIR or ""
local function P(n) return D .. n end
local function r(...) local t = {} for i = 1, select("#", ...) do t[#t+1] = tostring((select(i, ...))) end return table.concat(t, ",") end
local function esc(s) return (tostring(s):gsub("[%c\128-\255\\]", function(c) return string.format("\\%d", c:byte()) end)) end
local NAMES = {
  {"utf8-eacute", "\195\169.txt"}, {"latin1-eacute", "\233.txt"}, {"utf8-euro", "\226\130\172.txt"}, {"latin1-euro", "\128.txt"},
  {"byte81", "\129.txt"}, {"byte8d", "\141.txt"}, {"byte9d", "\157.txt"}, {"utf8-ri", "\230\151\165.txt"}, {"bytes-ff", "\255.txt"},
  {"utf8-eacute-dir", "d\195\169/x.txt"},
}
for _, n in ipairs(NAMES) do
  local f, e, c = io.open(P("w-" .. n[2]), "w")
  if f then f:write(n[1]) f:close() end
  print("create", n[1], esc("w-" .. n[2]), f and "ok" or r(e, c))
end
for _, n in ipairs(NAMES) do
  local f, e, c = io.open(P("x-" .. n[2]), "r")
  print("open existing", n[1], esc("x-" .. n[2]), f and ("ok:" .. r(f:read("*a"))) or r(e, c))
  if f then f:close() end
end
for _, n in ipairs(NAMES) do
  print("remove", n[1], r(os.remove(P("x-" .. n[2]))))
  print("rename", n[1], r(os.rename(P("w-" .. n[2]), P("r-" .. n[2]))))
  print("loadfile", n[1], r(loadfile(P("l-" .. n[2]))))
end
print("getenv", esc(os.getenv("NONASCII")))
print("getenv-ri", esc(os.getenv("NONASCII_RI")))
print("tmpname", esc(os.tmpname()))
print("getenv-utf8name", esc(os.getenv("V\195\169")), esc(os.getenv("V\233")))
