-- Bench: Empty function call overhead (Lua)
local function one() return 1 end

local iters = 1000000
local t0 = os.clock()
local acc = 0
for _ = 1, iters do
  acc = acc + one()
end
local t1 = os.clock()
local elapsed_ms = (t1 - t0) * 1000
print(string.format("empty_func_call: iters=%d, acc=%d, elapsed=%.1fms", iters, acc, elapsed_ms))
