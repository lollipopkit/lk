-- Bench: Iterative Fibonacci (Lua)
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
local t0 = os.clock()
local acc = 0
for _ = 1, iters do
  acc = fib_iterative(n)
end
local t1 = os.clock()
local elapsed_ms = (t1 - t0) * 1000
print(string.format("fib_iterative: fib(%d) = %d, iters=%d, elapsed=%.1fms", n, acc, iters, elapsed_ms))