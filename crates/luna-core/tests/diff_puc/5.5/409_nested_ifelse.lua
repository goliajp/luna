-- v2.15 P2.4: deep if-elseif chain.
local function grade(x)
  if x >= 90 then return "A"
  elseif x >= 80 then return "B"
  elseif x >= 70 then return "C"
  elseif x >= 60 then return "D"
  elseif x >= 50 then return "E"
  else return "F" end
end
for _, s in ipairs({95, 85, 75, 65, 55, 45}) do print(grade(s)) end
