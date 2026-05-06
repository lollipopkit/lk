-- Table operations benchmark
local start = os.clock()
local t = {}
for i = 1, 100000 do
    t[i] = i * 2
end
local sum = 0
for k, v in pairs(t) do
    sum = sum + v
end
local elapsed = os.clock() - start
print(string.format("lua table 100k: sum=%d, time=%.4fs", sum, elapsed))