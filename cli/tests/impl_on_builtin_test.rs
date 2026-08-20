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

/// The same list again, reached through a *trait* parameter rather than by
/// naming the method on the value.
///
/// A different path — the checker has to accept the argument where the trait is
/// declared, which asks the registry whether the type implements it — and it
/// was wrong for three of the thirteen. `List` and the other containers keyed
/// on their element (`List<Int>` against a registration of `List<Any>`);
/// `Stream` keyed the other way round, a bare name against a written-out
/// registration; and `Nil` never reached the question at all, because "a
/// nullable value does not fit a slot that is not nullable" answered first —
/// true of a slot, and not of a trait somebody wrote `impl D for Nil` for.
#[test]
fn a_trait_implemented_for_a_built_in_type_accepts_a_value_of_it() {
    let dir = tempfile::tempdir().expect("temp dir");
    let source = dir.path().join("trait_impls.lk");
    std::fs::write(
        &source,
        r#"use stream;

trait Tag { fn tag(self) -> String; }

impl Tag for Nil { fn tag(self) -> String { return "nil"; } }
impl Tag for Bool { fn tag(self) -> String { return "bool"; } }
impl Tag for Int { fn tag(self) -> String { return "int"; } }
impl Tag for Float { fn tag(self) -> String { return "float"; } }
impl Tag for String { fn tag(self) -> String { return "string"; } }
impl Tag for List { fn tag(self) -> String { return "list"; } }
impl Tag for Map { fn tag(self) -> String { return "map"; } }
impl Tag for Set { fn tag(self) -> String { return "set"; } }
impl Tag for Bytes { fn tag(self) -> String { return "bytes"; } }
impl Tag for Slice { fn tag(self) -> String { return "slice"; } }
impl Tag for Task { fn tag(self) -> String { return "task"; } }
impl Tag for Channel { fn tag(self) -> String { return "channel"; } }
impl Tag for Stream { fn tag(self) -> String { return "stream"; } }

fn name(v: Tag) -> String { return v.tag(); }

let nothing = nil;
let c = chan(1);
send(c, "x");
println([
  name(nothing),
  name(true),
  name(1),
  name(1.5),
  name("s"),
  name([1]),
  name({"k": 1}),
  name(Set([1])),
  name("ab".bytes()),
  name([1, 2, 3].slice(0, 2)),
  name(spawn(|| 1)),
  name(c),
  name(stream.range(0, 2)),
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
