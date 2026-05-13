-- Bench: Closure dispatch without captured values (Lua)
local inc = function(x) return x + 1 end

local iters = 1000000
local t0 = os.clock()
local acc = 0
for _ = 1, iters do
  acc = inc(acc)
end
local t1 = os.clock()
local elapsed_ms = (t1 - t0) * 1000
print(string.format("closure_no_capture: iters=%d, acc=%d, elapsed=%.1fms", iters, acc, elapsed_ms))
