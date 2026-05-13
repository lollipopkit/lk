-- Bench: Function call overhead (Lua)
local function add(a, b) return a + b end

local iters = 100000
local t0 = os.clock()
local acc = 0
for _ = 1, iters do
  acc = add(acc, 1)
end
local t1 = os.clock()
local elapsed_ms = (t1 - t0) * 1000
print(string.format("func_call: iters=%d, acc=%d, elapsed=%.1fms", iters, acc, elapsed_ms))