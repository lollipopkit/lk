-- Empty loop overhead benchmark
local start = os.clock()
local i = 0
while i < 1000000 do
    i = i + 1
end
local elapsed = os.clock() - start
print(string.format("lua empty loop 1M: time=%.4fs", elapsed))