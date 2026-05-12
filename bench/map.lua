-- Map/table string-key operations benchmark
local start = os.clock()
local m = {}
for i = 0, 9999 do
    local key = "k" .. i
    m[key] = i * 2
end
local sum = 0
for j = 0, 9999 do
    local key = "k" .. j
    sum = sum + m[key]
end
local elapsed = os.clock() - start
print(string.format("lua map 10k: sum=%d, time=%.4fs", sum, elapsed))
