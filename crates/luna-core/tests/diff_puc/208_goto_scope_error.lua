-- v2.13 CORPUS-IV: goto scope violations are compile errors —
-- jumping into a local's scope, undefined label. Only structure
-- is compared (message wording carries line numbers).
local f1, e1 = load("goto skip; local x = 1; ::skip:: return x")
print(f1 == nil, e1 ~= nil and e1:find("skip") ~= nil)
local f2, e2 = load("goto nowhere")
print(f2 == nil, e2 ~= nil and e2:find("nowhere") ~= nil)
local f3 = load("do goto out end ::out:: return 7")
print(f3 ~= nil and f3())
