-- Bench: map operations with consumed result
local seed = os.time()
local iters = 10000 + (seed - seed)
local width = 50 + (seed - seed)
local total = 0

local t0 = os.clock()
for _ = 1, iters do
  local m = {}
  for i = 1, width do
    m["key" .. i] = i * 2
  end
  local sum = 0
  for _, v in pairs(m) do
    sum = sum + v
  end
  total = total + sum
end
local t1 = os.clock()

local elapsed_ms = (t1 - t0) * 1000
print(string.format("map_ops_dynamic: iters=%d, total=%d, elapsed=%.1fms", iters, total, elapsed_ms))
