-- Arithmetic loop benchmark
local start = os.clock()
local total = 0
local i = 0
while i < 1000000 do
    local step = i + 1
    total = total + step * (step + 1)
    i = i + 1
end
local elapsed = os.clock() - start
print(string.format("lua arith 1M: total=%d, time=%.4fs", total, elapsed))