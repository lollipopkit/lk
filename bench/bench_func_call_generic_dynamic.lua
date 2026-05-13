-- Bench: generic dynamic function calls that should not match tiny-call fusion
local function mix(a, b, c)
  return (((a * 3) + (b % 11)) + (c * 5)) % 1000000007
end

local seed = os.time()
local iters = 100000 + (seed - seed)
local acc = 1

local t0 = os.clock()
for i = 1, iters do
  acc = mix(acc, i, i % 17)
end
local t1 = os.clock()

local elapsed_ms = (t1 - t0) * 1000
print(string.format("func_call_generic_dynamic: iters=%d, acc=%d, elapsed=%.3fms", iters, acc, elapsed_ms))
