-- v2.14 CV.3: yielding from the main "coroutine" fails.
print(pcall(coroutine.yield))
