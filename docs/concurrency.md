# Concurrency

LK's concurrency model is Go's, with one deliberate difference: **isolate
semantics** instead of shared memory. Goroutines run truly in parallel
(tokio multi-thread runtime underneath), communicate over channels, and
multiplex with `select` — but every value that crosses a goroutine boundary
is a deep copy. There is no shared mutable state and therefore no data race,
by construction ("share memory by communicating", enforced).

## Goroutines: `go` and `spawn`

```lk
go f(x);                  // fire-and-forget; f and x snapshotted now
let t = spawn(|| f(x));   // same, but keeps the Task handle
use task;
let result = task.await(t);   // blocks until done; raises if the task failed
```

- `go <expr>;` is parse-time sugar for `spawn(|| <expr>);` with the handle
  discarded.
- `spawn(f)` accepts any function or closure. At spawn time the closure's
  captures **and the current globals** (so goroutines can call named
  functions) are deep-copied into the goroutine's own private heap.
- **Snapshot semantics**: mutations inside a goroutine never leak back to
  the spawner — `let n = 0; go bump_n(); …` leaves the spawner's `n`
  untouched. Results come back through `task.await` or a channel.
- `task.try_await(t)` → the value once resolved, `nil` while running
  (pairs with postfix `!` to assert completion).

## Channels

Channels are the one shared thing (Arc-backed, not copied); the *values*
sent through them are deep copies.

```lk
let c = chan(8);        // capacity 8; chan(0) = unbounded
send(c, v);             // blocking; raises once c is closed
let v = recv(c);        // blocking; raises once c is closed AND drained
use chan as ch;
ch.try_send(c, v);      // -> Bool (false = full, not an error); closed raises
let v = ch.try_recv(c); // -> value | nil when empty; closed+drained raises
ch.close(c);            // Go close: buffered values stay receivable
ch.is_closed(c); ch.len(c); ch.capacity(c);
```

Failure semantics follow the v2 error model (see `docs/semantics.md`):
errors **raise** and are caught with try/catch — there are no `[ok, value]`
status pairs. Closing follows Go: `close` marks the channel and drops the
sender; the receiver drains remaining buffered values first, and only then
does `recv` raise. The standard drain loop:

```lk
try {
    while (true) {
        handle(recv(c));
    }
} catch e {
    // channel closed and drained
}
```

Blocking `send`/`recv` are goroutine-safe: called from inside a goroutine
they block only that goroutine's worker (tokio `block_in_place`), never the
whole runtime.

## `select`

Go-style multi-way channel choice, as an expression (full semantics in
`docs/semantics.md`; it is parse-time sugar over a hidden runtime builtin):

```lk
let got = select {
    case v <- recv(c) if enabled => v + 1;
    case send(out, 7) => "sent";
    default => "nothing-ready";     // omit to block
};
```

- Channel operands, send values, and guards evaluate eagerly, once, in
  source order.
- A **closed** channel is always ready (Go): its recv arm fires with the
  drained value, then with a `nil` binding — shutdown is observable through
  select.
- Without `default`, select blocks the current goroutine until an arm is
  ready.

## Differences from Go (all deliberate)

| | Go | LK |
|---|---|---|
| goroutine 内存 | 共享，靠约定避免竞争 | isolate：捕获/传值皆深拷贝，无竞争 |
| send on closed | panic | raise（可 try/catch） |
| recv on closed | 排空后返回零值+false | 排空后 raise；select arm 给 nil |
| 空 select{} | 死锁 panic | 全 guard 禁用且无 default → nil |
| goroutine 泄漏 | 可能 | 相同（阻塞的 goroutine 不会被回收）— 一样要自己收尾 |

The isolate model is the load-bearing architectural decision: the VM's GC
stays single-threaded and lock-free (the interpreter hot path), and no LK
program can express a data race. The cost is that a spawn deep-copies its
captures/globals and channel sends deep-copy payloads — pass handles
(channels, tasks) rather than large structures where it matters.

`examples/general/concurrency_demo.lk` and the `stdlib/src/spawn_test.rs` /
`chan_semantics_test.rs` suites are the runnable corpus.
