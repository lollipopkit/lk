#!/bin/bash
# Benchmark script for LKR peephole optimization experiments
# Measures empty loop, arithmetic, and fibonacci performance

LKR=./target/release/lkr
LUA=lua5.4

echo "=== LKR empty_loop ==="
RESULT=$($LKR bench/empty_loop.lkr 2>&1)
echo "$RESULT"
# Parse: "lkr empty loop 1M: time=0.018506484s"
TIME=$(echo "$RESULT" | grep -oP 'time=\K[0-9.]+')
echo "METRIC empty_loop_us=$(echo "$TIME * 1000000" | bc -l | cut -d. -f1)"

echo "=== LKR arith ==="
RESULT=$($LKR bench/arith.lkr 2>&1)
echo "$RESULT"
TIME=$(echo "$RESULT" | grep -oP 'time=\K[0-9.]+')
echo "METRIC arith_us=$(echo "$TIME * 1000000" | bc -l | cut -d. -f1)"

echo "=== LKR fib ==="
RESULT=$($LKR bench/fib.lkr 2>&1)
echo "$RESULT"
TIME=$(echo "$RESULT" | grep -oP 'time=\K[0-9.]+')
echo "METRIC fib_us=$(echo "$TIME * 1000000" | bc -l | cut -d. -f1)"

echo "=== Lua empty_loop ==="
RESULT=$($LUA bench/empty_loop.lua 2>&1)
echo "$RESULT"
TIME=$(echo "$RESULT" | grep -oP 'time=\K[0-9.]+')
echo "METRIC lua_empty_loop_us=$(echo "$TIME * 1000000" | bc -l | cut -d. -f1)"

echo "=== Lua arith ==="
RESULT=$($LUA bench/arith.lua 2>&1)
echo "$RESULT"
TIME=$(echo "$RESULT" | grep -oP 'time=\K[0-9.]+')
echo "METRIC lua_arith_us=$(echo "$TIME * 1000000" | bc -l | cut -d. -f1)"

echo "=== Lua fib ==="
RESULT=$($LUA bench/fib.lua 2>&1)
echo "$RESULT"
TIME=$(echo "$RESULT" | grep -oP 'time=\K[0-9.]+')
echo "METRIC lua_fib_us=$(echo "$TIME * 1000000" | bc -l | cut -d. -f1)"
