-- Bench: Empty loop — measures pure loop / iteration overhead (Lua)
local iters = 100000
local t0 = os.clock()
local count = 0
for _ = 1, iters do
  count = count + 1
end
local t1 = os.clock()
local elapsed_ms = (t1 - t0) * 1000
print(string.format("empty_loop: iters=%d, count=%d, elapsed=%.1fms", iters, count, elapsed_ms))