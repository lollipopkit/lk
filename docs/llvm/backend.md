# LLVM Backend

The current LLVM target emits a small runtime shell around the canonical
Instr32 module artifact. `lk compile llvm FILE.lk` writes `FILE.ll` containing
the serialized `.lkm` payload and a `main` function that calls
`lk_rt_run_module32_json`.

Native executable output is an artifact launcher over the same VM payload:

```sh
lk compile exe FILE.lk
```

emits a host executable that embeds the serialized `Module32Artifact` and runs
it through the `RuntimeVal` / `HeapStore` VM path. This is not the removed LKB
or old native callable bridge; it requires `rustc` and links against the current
`lk_core` / `lk_stdlib` rlibs from the active target directory.

Future native AOT can replace this launcher only if it stays on top of
`Module32Artifact`, `RuntimeVal`, and `HeapStore`; the removed LKB and old
callable/container bridges must not be reintroduced.
