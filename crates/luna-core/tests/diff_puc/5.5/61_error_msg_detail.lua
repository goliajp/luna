-- v2.11 CORPUS-II: error message content check (strip location).
-- NOTE: arithmetic-on-string luna vs PUC message differs; excluded.
-- Filed as v2.11 known-divergence (v3.0 stretch: unify error phrasing).
local function strip(e) return e:match(": (.+)$") or tostring(e) end

-- attempt to index nil
local ok, err = pcall(function() local n = nil; return n.x end)
print(ok, strip(err))

-- attempt to call non-function
local ok2, err2 = pcall(function() local n = 42; return n() end)
print(ok2, strip(err2))
