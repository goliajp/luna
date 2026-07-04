-- v2.13 CORPUS-IV: integration — sieve of Eratosthenes exercises
-- tables, numeric for, arithmetic, and string building together.
local N = 100
local sieve = {}
for i = 2, N do sieve[i] = true end
for i = 2, math.floor(math.sqrt(N)) do
  if sieve[i] then
    for j = i * i, N, i do sieve[j] = nil end
  end
end
local primes = {}
for i = 2, N do
  if sieve[i] then primes[#primes + 1] = i end
end
print(#primes)
print(table.concat(primes, ","))
print(primes[#primes])
