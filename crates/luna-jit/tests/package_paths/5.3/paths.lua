-- the default package paths, which on Windows name the interpreter's
-- directory, and the configuration
print(package.path)
print(package.cpath)
print((package.config:gsub("\n", "|")))
