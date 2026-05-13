-- Bench: dynamic counted loop with consumed result
local seed = os.time()
local iters = 1000000 + (seed - seed)
local count = 0

local t0 = os.clock()
for i = 1, iters do
  count = count + 1
end
local t1 = os.clock()

local elapsed_ms = (t1 - t0) * 1000
print(string.format("empty_loop_dynamic: iters=%d, count=%d, elapsed=%.1fms", iters, count, elapsed_ms))
