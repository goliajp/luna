-- v2.14 HD 5.4 seed: generational GC mode arrives; mode switches
-- report the previous mode.
print(collectgarbage("generational"))
print(collectgarbage("incremental"))
print(collectgarbage("incremental"))
collectgarbage()
print(collectgarbage("isrunning"))
