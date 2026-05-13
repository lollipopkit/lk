-- Bench: function call overhead with runtime-known argument
local function add(a, b)
  return a + b
end

local seed = os.time()
local iters = 100000 + (seed - seed)
local step = 1 + (seed - seed)
local acc = 0

local t0 = os.clock()
for _ = 1, iters do
  acc = add(acc, step)
end
local t1 = os.clock()

local elapsed_ms = (t1 - t0) * 1000
print(string.format("func_call_dynamic: iters=%d, acc=%d, elapsed=%.1fms", iters, acc, elapsed_ms))
