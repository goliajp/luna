-- v2.15 P2.5: generational GC mode.
collectgarbage("generational")
print(collectgarbage("isrunning"))
collectgarbage("incremental")
print(collectgarbage("isrunning"))
