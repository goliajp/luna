-- v3.1 debug slice: 5.5 names a nil error object once it is raised, for a
-- coroutine's resume as for pcall.
print(coroutine.resume(coroutine.create(function() error() end)))
print(pcall(error))
print(pcall(coroutine.wrap(function() error() end)))
