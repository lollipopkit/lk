-- Bench: Map operations — create, lookup, iterate (Lua)
local iters = 10000
local t0 = os.clock()
for _ = 1, iters do
  local m = {}
  for i = 1, 50 do
    m["key" .. i] = i * 2
  end
  local sum = 0
  for _, v in pairs(m) do
    sum = sum + v
  end
end
local t1 = os.clock()
local elapsed_ms = (t1 - t0) * 1000
print(string.format("map_ops: iters=%d, elapsed=%.1fms", iters, elapsed_ms))