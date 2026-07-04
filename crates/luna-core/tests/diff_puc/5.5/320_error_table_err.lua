-- v2.14 CV.2: a table error object with no __tostring reaches
-- the top as "(error object is a table value)" — no position.
error({ code = 42 })
