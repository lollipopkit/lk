-- Bench: dynamic numeric loop with per-iteration varying work
local seed = os.time()
local iters = 1000000 + (seed - seed)
local acc = 0

local t0 = os.clock()
for i = 1, iters do
  acc = acc + (i % 7)
end
local t1 = os.clock()

local elapsed_ms = (t1 - t0) * 1000
print(string.format("numeric_loop_varying_dynamic: iters=%d, acc=%d, elapsed=%.3fms", iters, acc, elapsed_ms))
