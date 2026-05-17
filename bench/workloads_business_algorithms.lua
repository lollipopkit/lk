-- Business-style algorithm workloads for LK vs Lua comparison.
-- Each block measures runtime only and prints: workload|name|checksum=...|elapsed=...ms

local seed = os.time()

local function emit(name, checksum, t0, t1)
  print(string.format("workload|%s|checksum=%d|elapsed=%.3fms", name, checksum, (t1 - t0) * 1000))
end

local function gcd(a, b)
  while b ~= 0 do
    local t = a % b
    a = b
    b = t
  end
  return a
end

local t0 = os.clock()
local checksum = 0
local gcd_iters = 80000 + (seed - seed)
for i = 1, gcd_iters do
  checksum = checksum + gcd((i * 37) + 11, (i * 19) + 7)
end
local t1 = os.clock()
emit("gcd_batch", checksum, t0, t1)

local function is_prime(n)
  if n < 2 then return false end
  if n == 2 then return true end
  if (n % 2) == 0 then return false end
  local d = 3
  while (d * d) <= n do
    if (n % d) == 0 then return false end
    d = d + 2
  end
  return true
end

t0 = os.clock()
checksum = 0
local prime_limit = 7000 + (seed - seed)
for n = 2, prime_limit do
  if is_prime(n) then
    checksum = checksum + n
  end
end
t1 = os.clock()
emit("prime_trial_division", checksum, t0, t1)

local function binary_search_implicit(target, n)
  local lo = 0
  local hi = n - 1
  while lo <= hi do
    local mid = math.floor((lo + hi) / 2)
    local value = mid * 2
    if value == target then
      return mid
    end
    if value < target then
      lo = mid + 1
    else
      hi = mid - 1
    end
  end
  return -1
end

t0 = os.clock()
checksum = 0
local search_iters = 120000 + (seed - seed)
local search_n = 4096 + (seed - seed)
for i = 1, search_iters do
  local target = (i % search_n) * 2
  checksum = checksum + binary_search_implicit(target, search_n)
end
t1 = os.clock()
emit("binary_search", checksum, t0, t1)

t0 = os.clock()
checksum = 0
local two_sum_rounds = 2500 + (seed - seed)
local two_sum_width = 80 + (seed - seed)
for r = 1, two_sum_rounds do
  local seen = {}
  local found = 0
  local target = two_sum_width + 1
  for i = 1, two_sum_width do
    seen["n" .. i] = i + r
  end
  for i = 1, two_sum_width do
    local need = target - i
    if seen["n" .. need] ~= nil then
      found = found + 1
    end
  end
  checksum = checksum + found
end
t1 = os.clock()
emit("two_sum_map", checksum, t0, t1)

t0 = os.clock()
checksum = 0
local window_iters = 4000 + (seed - seed)
local window_n = 120 + (seed - seed)
local window_size = 12 + (seed - seed)
for r = 1, window_iters do
  local values = {}
  for i = 0, window_n - 1 do
    values[#values + 1] = ((i * 31) + r) % 251
  end
  local rolling = 0
  for i = 0, window_n - 1 do
    rolling = rolling + values[i + 1]
    if i >= window_size then
      rolling = rolling - values[(i - window_size) + 1]
    end
    if i >= (window_size - 1) then
      checksum = checksum + rolling
    end
  end
end
t1 = os.clock()
emit("sliding_window_sum", checksum, t0, t1)

t0 = os.clock()
checksum = 0
local mat_iters = 18000 + (seed - seed)
for r = 1, mat_iters do
  local a00 = (r % 13) + 1; local a01 = 2; local a02 = 3
  local a10 = 4; local a11 = (r % 17) + 5; local a12 = 6
  local a20 = 7; local a21 = 8; local a22 = (r % 19) + 9
  local b00 = 3; local b01 = (r % 11) + 1; local b02 = 5
  local b10 = 7; local b11 = 9; local b12 = (r % 7) + 2
  local b20 = 4; local b21 = 6; local b22 = 8
  checksum = checksum + (a00 * b00) + (a01 * b10) + (a02 * b20)
  checksum = checksum + (a10 * b01) + (a11 * b11) + (a12 * b21)
  checksum = checksum + (a20 * b02) + (a21 * b12) + (a22 * b22)
end
t1 = os.clock()
emit("matrix_3x3_multiply", checksum, t0, t1)

t0 = os.clock()
checksum = 0
local stock_rounds = 3000 + (seed - seed)
local stock_n = 180 + (seed - seed)
for r = 1, stock_rounds do
  local min_price = 1000000
  local best = 0
  for i = 1, stock_n do
    local price = ((i * 97) + (r * 13)) % 1009
    if price < min_price then
      min_price = price
    end
    local profit = price - min_price
    if profit > best then
      best = profit
    end
  end
  checksum = checksum + best
end
t1 = os.clock()
emit("stock_max_profit", checksum, t0, t1)

t0 = os.clock()
checksum = 0
local hist_rounds = 3500 + (seed - seed)
local hist_n = 90 + (seed - seed)
for r = 1, hist_rounds do
  local hist = {}
  for i = 1, hist_n do
    local bucket = ((i * 17) + r) % 32
    local key = "b" .. bucket
    local prev = hist[key]
    if prev == nil then
      hist[key] = 1
    else
      hist[key] = prev + 1
    end
  end
  for b = 0, 31 do
    local v = hist["b" .. b]
    if v ~= nil then
      checksum = checksum + (v * v)
    end
  end
end
t1 = os.clock()
emit("histogram_group_count", checksum, t0, t1)

t0 = os.clock()
checksum = 0
local hash_rounds = 5000 + (seed - seed)
for r = 1, hash_rounds do
  local s = "tenant-" .. r .. "-order-" .. (r % 97) .. "-region-" .. (r % 11)
  local h = 2166136261
  for _ in s:gmatch(".") do
    h = ((h * 16777619) + 1) % 1000000007
  end
  checksum = checksum + h
end
t1 = os.clock()
emit("string_key_hash", checksum, t0, t1)

local function score_order(price, qty, discount)
  local subtotal = price * qty
  local fee = (subtotal % 17) + 3
  return subtotal + fee - discount
end

t0 = os.clock()
checksum = 0
local pipeline_iters = 90000 + (seed - seed)
for i = 1, pipeline_iters do
  checksum = checksum + score_order((i % 101) + 1, (i % 7) + 1, i % 13)
end
t1 = os.clock()
emit("order_score_pipeline", checksum, t0, t1)

t0 = os.clock()
checksum = 0
local log_rounds = 2200 + (seed - seed)
local log_width = 72 + (seed - seed)
for r = 1, log_rounds do
  local status_counts = {}
  for i = 1, log_width do
    local status = "ok"
    if ((i + r) % 17) == 0 then
      status = "error"
    elseif ((i + r) % 5) == 0 then
      status = "warn"
    end
    local line = "ts=" .. r .. "|tenant=t" .. (i % 13) .. "|status=" .. status .. "|path=/api/v1/orders/" .. (i % 19)
    local fields = {}
    for part in string.gmatch(line, "([^|]+)") do
      fields[#fields + 1] = part
    end
    local parsed_len = #table.concat(fields, "|")
    if string.find(line, "/api/v1", 1, true) ~= nil then
      checksum = checksum + (parsed_len % 7)
      local prev = status_counts[status]
      if prev == nil then
        status_counts[status] = 1
      else
        status_counts[status] = prev + 1
      end
    end
  end
  local ok_count = status_counts["ok"]
  local warn_count = status_counts["warn"]
  local error_count = status_counts["error"]
  if ok_count ~= nil then checksum = checksum + ok_count end
  if warn_count ~= nil then checksum = checksum + (warn_count * 7) end
  if error_count ~= nil then checksum = checksum + (error_count * 31) end
end
t1 = os.clock()
emit("log_parse_filter", checksum, t0, t1)

local function cart_line_total(sku, qty, region, prices, tax_rates)
  local price = prices[sku]
  local tax = tax_rates[region]
  local subtotal = price * qty
  local discount = 0
  if qty >= 5 then
    discount = math.floor(subtotal / 10)
  elseif string.sub(sku, 1, 3) == "pro" then
    discount = math.floor(subtotal / 20)
  end
  return subtotal - discount + math.floor((subtotal * tax) / 100)
end

t0 = os.clock()
checksum = 0
local cart_rounds = 5000 + (seed - seed)
local prices = {basic = 19, pro = 49, team = 99, addon = 7}
local tax_rates = {us = 8, eu = 20, apac = 12}
for r = 1, cart_rounds do
  local region = "us"
  if (r % 3) == 1 then
    region = "eu"
  elseif (r % 3) == 2 then
    region = "apac"
  end
  checksum = checksum + cart_line_total("basic", (r % 6) + 1, region, prices, tax_rates)
  checksum = checksum + cart_line_total("pro", (r % 4) + 1, region, prices, tax_rates)
  checksum = checksum + cart_line_total("team", (r % 3) + 1, region, prices, tax_rates)
  if (r % 2) == 0 then
    checksum = checksum + cart_line_total("addon", (r % 8) + 1, region, prices, tax_rates)
  end
end
t1 = os.clock()
emit("cart_pricing_rules", checksum, t0, t1)

t0 = os.clock()
checksum = 0
local route_rounds = 90000 + (seed - seed)
local role_levels = {guest = 0, user = 10, analyst = 40, ops = 70, admin = 100}
for i = 1, route_rounds do
  if (i % 11) == 0 then
    checksum = checksum + (i % 97) + role_levels["admin"] + #"/admin/users"
  elseif (i % 7) == 0 then
    checksum = checksum + (i % 97) + role_levels["ops"] + #"DELETE"
  elseif (i % 5) == 0 then
    checksum = checksum + (i % 97) + role_levels["analyst"] + #"/reports/daily"
  elseif (i % 2) == 0 then
    checksum = checksum + (i % 97) + role_levels["user"] + #"/api/orders"
  else
    checksum = checksum + 3 + role_levels["guest"]
  end
end
t1 = os.clock()
emit("route_permission_check", checksum, t0, t1)

t0 = os.clock()
checksum = 0
local inventory_rounds = 2800 + (seed - seed)
for r = 1, inventory_rounds do
  local stock = {}
  local reorder = {}
  for i = 1, 64 do
    local sku = "sku-" .. (i % 23)
    local current = stock[sku]
    local delta = ((i * 11) + r) % 37
    if current == nil then
      stock[sku] = delta
    else
      stock[sku] = current + delta
    end
  end
  for i = 0, 22 do
    local sku = "sku-" .. i
    local available = stock[sku]
    if available == nil then
      available = 0
    end
    local demand = ((r + i * 7) % 41) + 5
    if available < demand then
      reorder[#reorder + 1] = sku
      checksum = checksum + ((demand - available) * (i + 1))
    else
      checksum = checksum + (available % 17)
    end
  end
  checksum = checksum + #table.concat(reorder, ",")
end
t1 = os.clock()
emit("inventory_reorder", checksum, t0, t1)

local function fraud_score(amount, country, device, prior_declines, risky_countries)
  local score = 0
  if amount > 900 then
    score = score + 40
  elseif amount > 400 then
    score = score + 15
  end
  if risky_countries[country] ~= nil then
    score = score + 35
  end
  if string.sub(device, 1, 3) == "emu" then
    score = score + 30
  end
  score = score + (prior_declines * 9)
  return score
end

t0 = os.clock()
checksum = 0
local fraud_rounds = 85000 + (seed - seed)
local risky_countries = {ng = true, ru = true, kp = true}
for i = 1, fraud_rounds do
  local country = "us"
  if (i % 29) == 0 then
    country = "ng"
  elseif (i % 31) == 0 then
    country = "ru"
  elseif (i % 7) == 0 then
    country = "de"
  end
  local device = "ios"
  if (i % 19) == 0 then
    device = "emu-android"
  elseif (i % 5) == 0 then
    device = "web"
  end
  local amount = ((i * 37) % 1200) + 1
  local score = fraud_score(amount, country, device, i % 4, risky_countries)
  if score >= 70 then
    checksum = checksum + (score * 2)
  else
    checksum = checksum + score + 1
  end
end
t1 = os.clock()
emit("fraud_rule_scoring", checksum, t0, t1)
