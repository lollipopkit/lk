-- Bench: Create no-capture closure and call it once per loop
local iters = 100000
local t0 = os.clock() * 1000
local acc = 0
for _ = 1, iters do
    local inc = function(x) return x + 1 end
    acc = inc(acc)
end
local t1 = os.clock() * 1000
print(string.format("closure_create_no_capture: iters=%d, acc=%d, elapsed=%.1fms", iters, acc, t1 - t0))
