-- what require keeps on its stack below a preload function, a searcher
-- and the loader a searcher returns
local function show(tag)
  local out = {}
  for i = 1, 14 do
    local name, v = debug.getlocal(3, i)
    if not name then break end
    local tv = type(v)
    out[#out + 1] = (tv == "string" or tv == "number" or tv == "boolean") and tostring(v) or tv
  end
  print(tag, #out, table.concat(out, " "))
end
package.preload.pre = function(...) show("preload") return 1 end
require("pre")
local searchers = package.searchers or package.loaders
table.insert(searchers, 1, function(name)
  if name == "srch" then
    show("searcher")
    return function() show("loader") return 2 end, "extra"
  end
  return nil
end)
require("srch")
