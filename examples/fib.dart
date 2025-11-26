int fibIterative(int n) {
  if (n <= 1) return n;
  var a = 0;
  var b = 1;
  for (var i = 2; i <= n; i++) {
    final t = a + b;
    a = b;
    b = t;
  }
  return b;
}

void main(List<String> args) {
  final iters = 50000;
  final n = 30;

  var acc = 0;
  for (var i = 0; i < iters; i++) {
    acc = fibIterative(n);
  }
  print('fib($n) = $acc, iters=$iters');
}
