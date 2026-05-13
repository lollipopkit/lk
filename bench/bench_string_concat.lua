-- Bench: String concatenation (Lua)
local iters = 10000
local n = 100
local t0 = os.clock()
for _ = 1, iters do
  local s = ""
  for i = 1, n do
    s = s .. "x"
  end
end
local t1 = os.clock()
local elapsed_ms = (t1 - t0) * 1000
print(string.format("string_concat: iters=%d, n=%d, elapsed=%.1fms", iters, n, elapsed_ms))