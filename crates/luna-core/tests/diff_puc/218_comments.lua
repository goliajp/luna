-- v2.13 CORPUS-IV: comment forms.
local x = 1 -- line comment
--[[ block
comment ]]
local y = 2
--[==[ level-2 block with ]] inside ]==]
local z = 3
---[[ leading dashes make this a LINE comment, so code below runs
local w = 10
--]]
print(x, y, z, w)
