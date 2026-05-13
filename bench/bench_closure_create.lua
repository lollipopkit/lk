-- Bench: Closure creation plus one call per closure (Lua)
local function make_adder(n) return function(x) return x + n end end

local iters = 100000
local t0 = os.clock()
local acc = 0
for i = 1, iters do
  local adder = make_adder(i)
  acc = adder(acc)
end
local t1 = os.clock()
local elapsed_ms = (t1 - t0) * 1000
print(string.format("closure_create: iters=%d, acc=%d, elapsed=%.1fms", iters, acc, elapsed_ms))
