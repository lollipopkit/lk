-- Bench: iterative Fibonacci with runtime-known input
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

local seed = os.time()
local iters = 50000 + (seed - seed)
local n = 30 + (seed - seed)
local acc = 0

local t0 = os.clock()
for _ = 1, iters do
  acc = fib_iterative(n)
end
local t1 = os.clock()

local elapsed_ms = (t1 - t0) * 1000
print(string.format("fib_iterative_dynamic: fib(%d) = %d, iters=%d, elapsed=%.1fms", n, acc, iters, elapsed_ms))
