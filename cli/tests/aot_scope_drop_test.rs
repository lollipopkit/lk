//! Scope-drop regression: a loop that allocates a temporary container per
//! iteration must not grow the `lkrt` arena without bound.
//!
//! Container handles are arena-owned and reclaimed by `lkrt_cleanup()` at exit
//! (RFC aot-redesign §3.4), which is fine for straight-line code and wrong for
//! a loop: before `lk_aot_mir::opt`'s scope-drop pass, the program below held
//! every one of its 2,000,000 temporaries, measuring ~190 MB RSS against the
//! VM's ~8.8 MB. This test pins both halves of the fix — the output stays
//! VM-identical (no premature free) and the memory stays flat.

#![cfg(feature = "aot")]

use std::path::PathBuf;
use std::process::Command;

fn bin_path() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_lk"))
}

/// A loop whose body builds a list that dies at the end of the iteration.
const LOOP_TEMPORARIES: &str = "\
let n = 0;\n\
for i in 0..300000 {\n\
    let tmp = [i, i + 1, i + 2];\n\
    n += tmp.len();\n\
}\n\
println(n);\n";

/// Peak RSS in KiB of a finished child, read from `/proc` accounting via
/// `getrusage`-style reporting. Linux-only, which the test guards on.
#[cfg(target_os = "linux")]
fn peak_rss_kib(exe: &std::path::Path) -> u64 {
    // `/usr/bin/time -v` is not guaranteed present; run the child and read its
    // own peak from /proc/self/status by wrapping in a shell that reports it.
    let output = Command::new("sh")
        .arg("-c")
        .arg(format!(
            "{} >/dev/null & pid=$!; peak=0; \
             while kill -0 $pid 2>/dev/null; do \
               cur=$(awk '/VmHWM/ {{print $2}}' /proc/$pid/status 2>/dev/null); \
               [ -n \"$cur\" ] && [ \"$cur\" -gt \"$peak\" ] && peak=$cur; \
             done; wait $pid; echo $peak",
            exe.display()
        ))
        .output()
        .expect("run child under rss sampling");
    String::from_utf8_lossy(&output.stdout).trim().parse().unwrap_or(0)
}

#[test]
fn loop_local_containers_do_not_grow_the_arena() {
    let dir = std::env::temp_dir().join(format!("lk_scope_drop_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create tmp dir");
    let file = dir.join("loops.lk");
    std::fs::write(&file, LOOP_TEMPORARIES).expect("write program");

    let vm = Command::new(bin_path())
        .current_dir(&dir)
        .arg("loops.lk")
        .env("LK_FORCE_VM", "1")
        .output()
        .expect("vm run");
    assert!(vm.status.success(), "vm: {}", String::from_utf8_lossy(&vm.stderr));

    let compile = Command::new(bin_path())
        .current_dir(&dir)
        .args(["compile", "loops.lk"])
        // Pure native: a Tier 0 fallback would run the VM and prove nothing.
        .env("LK_AOT_HYBRID", "0")
        .env("LK_AOT_NO_FALLBACK", "1")
        .output()
        .expect("native compile");
    assert!(
        compile.status.success(),
        "compile: {}",
        String::from_utf8_lossy(&compile.stderr)
    );

    let exe = dir.join("loops");
    let native = Command::new(&exe).output().expect("native run");
    // Correctness first: releasing a handle that was still live would show up
    // as a wrong result or a crash long before it showed up as memory.
    assert_eq!(
        String::from_utf8_lossy(&vm.stdout),
        String::from_utf8_lossy(&native.stdout),
        "native output must match the VM"
    );
    assert!(native.status.success(), "native run failed");

    #[cfg(target_os = "linux")]
    {
        // Measured on this program: ~4.5 MiB with scope drop, ~36.5 MiB
        // without it (`LK_AOT_NO_OPT=1`). 16 MiB sits between the two with
        // room on both sides, so the assert catches the regression without
        // being sensitive to allocator details.
        let peak = peak_rss_kib(&exe);
        assert!(
            peak > 0 && peak < 16 * 1024,
            "peak RSS {peak} KiB suggests loop temporaries are being retained \
             (expected ~4.5 MiB; ~36.5 MiB is the un-dropped baseline)"
        );
    }

    let _ = std::fs::remove_dir_all(&dir);
}
