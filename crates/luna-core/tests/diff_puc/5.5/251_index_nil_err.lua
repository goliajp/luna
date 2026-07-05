-- v2.14 HC probe: top-level runtime error — both interpreters
-- must fail, with position prefix, same message (error channel).
local x = nil
return x.field
