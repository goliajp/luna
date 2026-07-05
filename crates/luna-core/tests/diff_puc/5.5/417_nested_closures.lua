-- v2.15 P2.4: nested closures capturing multiple levels.
local function outer(a)
  return function(b)
    return function(c)
      return function(d)
        return a + b + c + d
      end
    end
  end
end
print(outer(1)(2)(3)(4))
print(outer(10)(20)(30)(40))
