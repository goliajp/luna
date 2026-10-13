-- rule: ltable.luaH_next.1
-- v2.14 CV.2: next() with a key absent from the table.
return next({}, "no_such_key")
