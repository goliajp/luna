-- rule: loadlib.setpath.4 loadlib.searcher_C.1
-- package.cpath starts as PUC's default for the platform, and a module
-- found on no path lists the cpath candidates in the error
print(package.cpath)
package.path = "./no_such_dir/?.lua"
print(select(2, pcall(require, "no_such_mod")))
print(select(2, pcall(require, "no_such_mod.sub")))
