-- Function call overhead benchmark
local function add(a, b)
    return a + b
end

local start = os.clock()
local result = 0
for i = 1, 1000000 do
    result = add(result, 1)
end
local elapsed = os.clock() - start
print(string.format("lua call 1M: result=%d, time=%.4fs", result, elapsed))