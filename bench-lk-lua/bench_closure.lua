-- Bench: Closure / lambda call overhead (Lua)
local function make_adder(n) return function(x) return x + n end end

local adder = make_adder(1)
local iters = 100000
local t0 = os.clock()
local acc = 0
for _ = 1, iters do
  acc = adder(acc)
end
local t1 = os.clock()
local elapsed_ms = (t1 - t0) * 1000
print(string.format("closure_call: iters=%d, acc=%d, elapsed=%.1fms", iters, acc, elapsed_ms))