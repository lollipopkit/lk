-- Fibonacci benchmark (iterative)
local function fibonacci(n)
    if n <= 1 then return n end
    local a = 0
    local b = 1
    for i = 2, n do
        local t = a + b
        a = b
        b = t
    end
    return b
end

local start = os.clock()
local result
for i = 1, 100000 do
    result = fibonacci(30)
end
local elapsed = os.clock() - start
print(string.format("lua fib30 x100k: result=%d, time=%.4fs", result, elapsed))