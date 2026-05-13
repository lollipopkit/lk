-- Bench: recursive Fibonacci with runtime-known input
local function fib(n)
  if n <= 1 then return n end
  return fib(n - 1) + fib(n - 2)
end

local seed = os.time()
local iters = 5000 + (seed - seed)
local n = 15 + (seed - seed)
local acc = 0

local t0 = os.clock()
for _ = 1, iters do
  acc = fib(n)
end
local t1 = os.clock()

local elapsed_ms = (t1 - t0) * 1000
print(string.format("fib_recursive_dynamic: fib(%d) = %d, iters=%d, elapsed=%.1fms", n, acc, iters, elapsed_ms))
