-- rule: lgc.luaC_checkfinalizer.4
-- a finalizer run while the state closes registers another object: that
-- one is not finalized
keep = setmetatable({}, {__gc = function()
  print("outer finalizer")
  setmetatable({}, {__gc = function() print("inner finalizer") end})
end})
print("end of script")
