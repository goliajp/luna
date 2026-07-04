-- v2.14 CV.3: coroutine API argument type errors via pcall shape.
print(pcall(coroutine.resume, 42))
print(pcall(coroutine.status, "x"))
print((pcall(coroutine.create, 5)))
