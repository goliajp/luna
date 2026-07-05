-- v2.14 CV.2: table.concat with a non-string element.
return table.concat({ "a", {}, "c" })
