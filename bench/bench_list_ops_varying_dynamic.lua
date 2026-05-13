-- Bench: list operations with per-iteration changing contents
local seed = os.time()
local iters = 5000 + (seed - seed)
local width = 100 + (seed - seed)
local total = 0

local t0 = os.clock()
for outer = 1, iters do
  local list = {}
  for i = 1, width do
    list[#list + 1] = i + (outer % 5)
  end
  local sum = 0
  for _, v in ipairs(list) do
    sum = sum + v
  end
  total = total + sum
end
local t1 = os.clock()

local elapsed_ms = (t1 - t0) * 1000
print(string.format("list_ops_varying_dynamic: iters=%d, total=%d, elapsed=%.3fms", iters, total, elapsed_ms))
