//! Every built-in type a program can write an `impl` for dispatches to it.
//!
//! Four things have to agree about what a receiver's type *is*: the type
//! parser, the checker's dispatch key, the runtime's dispatch key, and the
//! scope the impl is filed in. They disagreed for five of the thirteen, each in
//! its own way — `Bytes` on the scope; `Slice` on the parse, both dispatch keys
//! and the scope; `Task`, `Channel` and `Stream` on the parse and the runtime
//! key, where a bare `Task` meant a task of *nothing* and matched no receiver.
//!
//! The symptom was identical every time: the impl compiled, and calling its
//! method said the value had no such method. So the list is walked whole here
//! rather than case by case — a type added to the language belongs in it, and
//! the failure it guards against is silent until someone writes that impl.
//!
//! A CLI test rather than a `core` one because `Stream` needs the standard
//! library, which `core`'s executor tests do not link.

/// The parameter is not part of the identity: an impl target names the
/// *constructor*, so a `Channel<String>` finds what `impl Channel` registered.
#[test]
fn an_impl_on_a_built_in_type_is_reachable_from_a_value_of_it() {
    let dir = tempfile::tempdir().expect("temp dir");
    let source = dir.path().join("impls.lk");
    std::fs::write(
        &source,
        r#"use stream;

impl Nil { fn tag(self) -> String { return "nil"; } }
impl Bool { fn tag(self) -> String { return "bool"; } }
impl Int { fn tag(self) -> String { return "int"; } }
impl Float { fn tag(self) -> String { return "float"; } }
impl String { fn tag(self) -> String { return "string"; } }
impl List { fn tag(self) -> String { return "list"; } }
impl Map { fn tag(self) -> String { return "map"; } }
impl Set { fn tag(self) -> String { return "set"; } }
impl Bytes { fn tag(self) -> String { return "bytes"; } }
impl Slice { fn tag(self) -> String { return "slice"; } }
impl Task { fn tag(self) -> String { return "task"; } }
impl Channel { fn tag(self) -> String { return "channel"; } }
impl Stream { fn tag(self) -> String { return "stream"; } }

let nothing = nil;
let c = chan(1);
send(c, "x");
println([
  nothing.tag(),
  true.tag(),
  (1).tag(),
  (1.5).tag(),
  "s".tag(),
  [1].tag(),
  {"k": 1}.tag(),
  Set([1]).tag(),
  "ab".bytes().tag(),
  [1, 2, 3].slice(0, 2).tag(),
  spawn(|| 1).tag(),
  c.tag(),
  stream.range(0, 2).tag(),
].join(","));
"#,
    )
    .expect("write source");

    let out = std::process::Command::new(env!("CARGO_BIN_EXE_lk"))
        .arg(source.to_str().expect("utf-8 path"))
        .env("LK_FORCE_VM", "1")
        .output()
        .expect("run under the VM");
    assert!(
        out.status.success(),
        "the program did not run: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        "nil,bool,int,float,string,list,map,set,bytes,slice,task,channel,stream"
    );
}
