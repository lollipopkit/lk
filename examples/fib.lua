local function fib_iterative(n)
  if n <= 1 then return n end
  local a, b = 0, 1
  for _ = 2, n do
    local t = a + b
    a = b
    b = t
  end
  return b
end

local iters = 50000
local n = 30

local acc = 0
for _ = 1, iters do
  acc = fib_iterative(n)
end

print(string.format("fib(%d) = %d, iters=%d", n, acc, iters))

