-- Bench: dynamic function calls with per-iteration changing arguments
local function mix(a, b)
  return (a + b) % 1000000007
end

local seed = os.time()
local iters = 100000 + (seed - seed)
local acc = 0

local t0 = os.clock()
for i = 1, iters do
  acc = mix(acc, i % 7)
end
local t1 = os.clock()

local elapsed_ms = (t1 - t0) * 1000
print(string.format("func_call_varying_dynamic: iters=%d, acc=%d, elapsed=%.3fms", iters, acc, elapsed_ms))
