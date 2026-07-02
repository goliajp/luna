-- v2.12 CORPUS-III: require on preloaded stdlib modules
-- resolves via package.loaded identity (no filesystem).
print(require("string") == string)
print(require("table") == table)
print(require("math") == math)
print(package.loaded.string == string)
print(type(package.loaded), type(package.preload))
