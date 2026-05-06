-- String concatenation benchmark
local start = os.clock()
local s = ""
for i = 1, 100000 do
    s = s .. "x"
end
local elapsed = os.clock() - start
print(string.format("lua strcat 100k: len=%d, time=%.4fs", #s, elapsed))