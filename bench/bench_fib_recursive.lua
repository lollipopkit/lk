-- Bench: Recursive Fibonacci (Lua)
local function fib(n)
  if n <= 1 then return n end
  return fib(n - 1) + fib(n - 2)
end

local iters = 5000
local n = 15
local t0 = os.clock()
local acc = 0
for _ = 1, iters do
  acc = fib(n)
end
local t1 = os.clock()
local elapsed_ms = (t1 - t0) * 1000
print(string.format("fib_recursive: fib(%d) = %d, iters=%d, elapsed=%.1fms", n, acc, iters, elapsed_ms))