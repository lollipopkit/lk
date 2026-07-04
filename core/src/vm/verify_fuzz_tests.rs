//! Decoder/verifier fuzz: `.lkm` artifacts are untrusted external input, and
//! the load path (`ModuleArtifact::from_json_str` → `into_module`, which runs
//! `verify::verify_module`) promises *clean rejection* — malformed input may
//! fail with `Err`, but must never panic, whatever the corruption.
//!
//! Three generators drive that promise: byte-level corruption of valid
//! artifact JSON, structure-aware mutation of the parsed JSON tree (valid
//! serde shapes with hostile values — out-of-bounds indices, huge lengths,
//! flipped fields — the layer the verifier exists for), and raw garbage.
//! A fixed corpus additionally pins the targeted attacks (entry out of
//! bounds, corrupt instruction words, zeroed register counts, fact-table
//! length bombs, deep-nesting JSON) deterministically.
//!
//! Deterministic by default; scale with `LK_FUZZ_CASES`, reseed with
//! `LK_FUZZ_SEED`. Failures print the payload for reproduction.

use std::panic::{AssertUnwindSafe, catch_unwind};

use serde_json::Value;

use super::{Compiler, ModuleArtifact};

/// splitmix64: tiny, deterministic, no external dependencies.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

/// Seed programs span the serialized surface: call windows and debug names,
/// for-loop facts (`ForLoopI` has no fact-less fallback), heap constants
/// (list/map/long string), closures with captures, and string method calls.
const SEED_PROGRAMS: &[&str] = &[
    "fn add(a, b) { return a + b; }\nreturn add(2, 3);\n",
    "let total = 0;\nfor i in 0..10 {\n    total += i;\n}\nreturn total;\n",
    "let xs = [1, 2, 3];\nlet m = {\"k0\": 1, \"k1\": 2};\nlet s = \"a long constant string that will not fit in a short-string slot\";\nreturn xs[1] + m[\"k1\"];\n",
    "let n = 4;\nlet f = |x| x * n + 1;\nreturn f(5);\n",
    "let x = 1.5;\nlet i = 0;\nwhile (i < 5) { x = x + 0.5; i = i + 1; }\nif (x > 3.0) { return x; }\nreturn 0.0;\n",
];

fn compile_artifact_json(source: &str) -> String {
    let tokens = crate::token::Tokenizer::tokenize(source).expect("tokenize seed");
    let program = crate::stmt::StmtParser::new(&tokens)
        .parse_program()
        .expect("parse seed");
    let module = Compiler::compile_module(&program).expect("compile seed");
    let artifact = ModuleArtifact::new(Vec::new(), &module).expect("seed artifact");
    artifact.to_json_string().expect("serialize seed artifact")
}

#[derive(PartialEq, Debug)]
enum Outcome {
    Accepted,
    Rejected,
}

fn decode(payload: &str) -> Outcome {
    match ModuleArtifact::from_json_str(payload).and_then(ModuleArtifact::into_module) {
        Ok(_) => Outcome::Accepted,
        Err(_) => Outcome::Rejected,
    }
}

/// The single assertion under test: decode + verify never panics.
fn decode_without_panicking(payload: &str, what: &str) -> Outcome {
    catch_unwind(AssertUnwindSafe(|| decode(payload))).unwrap_or_else(|_| {
        let head: String = payload.chars().take(2000).collect();
        panic!(
            "artifact decode/verify panicked on {what}\n--- payload ({} bytes, head) ---\n{head}\n---",
            payload.len()
        );
    })
}

// ---- generators -------------------------------------------------------------

fn mutate_bytes(rng: &mut Rng, seed: &str) -> String {
    let mut bytes = seed.as_bytes().to_vec();
    let ops = 1 + rng.below(16);
    for _ in 0..ops {
        if bytes.is_empty() {
            break;
        }
        match rng.below(5) {
            0 => {
                let i = rng.below(bytes.len() as u64) as usize;
                bytes[i] = rng.next() as u8;
            }
            1 => {
                let i = rng.below(bytes.len() as u64 + 1) as usize;
                bytes.insert(i, rng.next() as u8);
            }
            2 => {
                let i = rng.below(bytes.len() as u64) as usize;
                bytes.remove(i);
            }
            3 => {
                let i = rng.below(bytes.len() as u64) as usize;
                bytes.truncate(i);
            }
            _ => {
                let start = rng.below(bytes.len() as u64) as usize;
                let len = (rng.below(32) as usize).min(bytes.len() - start);
                let slice = bytes[start..start + len].to_vec();
                let at = rng.below(bytes.len() as u64 + 1) as usize;
                bytes.splice(at..at, slice);
            }
        }
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

#[derive(Clone)]
enum Seg {
    Key(String),
    Index(usize),
}

fn collect_paths(value: &Value, prefix: &mut Vec<Seg>, out: &mut Vec<Vec<Seg>>) {
    out.push(prefix.clone());
    match value {
        Value::Array(items) => {
            for (i, item) in items.iter().enumerate() {
                prefix.push(Seg::Index(i));
                collect_paths(item, prefix, out);
                prefix.pop();
            }
        }
        Value::Object(map) => {
            for (key, item) in map {
                prefix.push(Seg::Key(key.clone()));
                collect_paths(item, prefix, out);
                prefix.pop();
            }
        }
        _ => {}
    }
}

fn node_mut<'v>(root: &'v mut Value, path: &[Seg]) -> Option<&'v mut Value> {
    let mut node = root;
    for seg in path {
        node = match seg {
            Seg::Key(key) => node.get_mut(key.as_str())?,
            Seg::Index(i) => node.get_mut(*i)?,
        };
    }
    Some(node)
}

fn mutate_node(rng: &mut Rng, node: &mut Value) {
    const HOSTILE_NUMBERS: [i64; 8] = [0, 1, -1, 255, 65_535, 4_294_967_295, i64::MAX, i64::MIN];
    match node {
        Value::Number(_) => {
            *node = match rng.below(10) {
                0 => Value::from(u64::MAX),
                1 => Value::from(1e308),
                2 => Value::from(-1e308),
                n => Value::from(HOSTILE_NUMBERS[n as usize % HOSTILE_NUMBERS.len()]),
            };
        }
        Value::String(s) => match rng.below(4) {
            0 => s.clear(),
            1 => *s = "a".repeat(8192),
            2 => *s = "lk.module".to_string(),
            _ => *s = format!("junk-{}", rng.next()),
        },
        Value::Bool(b) => *b = !*b,
        Value::Null => *node = Value::from(rng.next()),
        Value::Array(items) => match rng.below(4) {
            0 if !items.is_empty() => {
                let i = rng.below(items.len() as u64) as usize;
                items.remove(i);
            }
            1 if !items.is_empty() => {
                let i = rng.below(items.len() as u64) as usize;
                let dup = items[i].clone();
                items.push(dup);
            }
            2 => items.clear(),
            _ => items.push(Value::from(rng.next())),
        },
        Value::Object(map) => {
            let keys: Vec<String> = map.keys().cloned().collect();
            match rng.below(3) {
                0 if !keys.is_empty() => {
                    let key = &keys[rng.below(keys.len() as u64) as usize];
                    map.remove(key);
                }
                1 if !keys.is_empty() => {
                    let key = &keys[rng.below(keys.len() as u64) as usize];
                    map[key] = Value::Null;
                }
                _ => {
                    map.insert("junk".to_string(), Value::from(u64::MAX));
                }
            }
        }
    }
}

fn mutate_json(rng: &mut Rng, seed: &str) -> Option<String> {
    let mut root: Value = serde_json::from_str(seed).ok()?;
    let mutations = 1 + rng.below(4);
    for _ in 0..mutations {
        let mut paths = Vec::new();
        collect_paths(&root, &mut Vec::new(), &mut paths);
        let path = paths[rng.below(paths.len() as u64) as usize].clone();
        mutate_node(rng, node_mut(&mut root, &path)?);
    }
    serde_json::to_string(&root).ok()
}

fn garbage(rng: &mut Rng) -> String {
    let len = rng.below(2048) as usize;
    let bytes: Vec<u8> = (0..len).map(|_| rng.next() as u8).collect();
    String::from_utf8_lossy(&bytes).into_owned()
}

// ---- tests -------------------------------------------------------------------

#[test]
fn fuzz_malformed_artifacts_are_rejected_without_panicking() {
    let cases: u64 = std::env::var("LK_FUZZ_CASES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(300);
    let seed: u64 = std::env::var("LK_FUZZ_SEED")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0x1D_C0DE);

    let corpus: Vec<String> = SEED_PROGRAMS.iter().map(|s| compile_artifact_json(s)).collect();
    // Pristine artifacts must decode: otherwise every mutation below would be
    // exercising an already-broken baseline instead of the verifier.
    for (i, json) in corpus.iter().enumerate() {
        assert_eq!(
            decode(json),
            Outcome::Accepted,
            "pristine seed artifact {i} must decode"
        );
    }

    let mut accepted = 0_u64;
    let mut rejected = 0_u64;
    let mut rng = Rng(seed);
    for case in 0..cases {
        let pick = rng.below(corpus.len() as u64) as usize;
        let (payload, what) = match rng.below(10) {
            0..=3 => (
                mutate_bytes(&mut rng, &corpus[pick]),
                format!("byte-mutated seed {pick} (case {case}, fuzz seed {seed:#x})"),
            ),
            4..=8 => {
                let Some(payload) = mutate_json(&mut rng, &corpus[pick]) else {
                    continue;
                };
                (
                    payload,
                    format!("json-mutated seed {pick} (case {case}, fuzz seed {seed:#x})"),
                )
            }
            _ => (garbage(&mut rng), format!("garbage (case {case}, fuzz seed {seed:#x})")),
        };
        match decode_without_panicking(&payload, &what) {
            Outcome::Accepted => accepted += 1,
            Outcome::Rejected => rejected += 1,
        }
    }

    println!("verifier fuzz: {rejected} rejected / {accepted} accepted over {cases} cases (seed {seed:#x})");
    // Mutations that never reject would mean the generator degraded into
    // producing only valid artifacts — the fuzz would no longer test rejection.
    assert!(
        rejected * 2 >= cases,
        "only {rejected}/{cases} mutated artifacts were rejected; the mutation generator has degraded"
    );
}

#[test]
fn targeted_hostile_artifacts_are_cleanly_rejected() {
    let base = compile_artifact_json(SEED_PROGRAMS[0]);
    let mutate = |pointer: &str, value: Value| -> String {
        let mut root: Value = serde_json::from_str(&base).expect("parse base artifact");
        *root.pointer_mut(pointer).expect("pointer resolves") = value;
        serde_json::to_string(&root).expect("serialize mutated artifact")
    };

    // Each of these must be *rejected* (not just non-panicking): they attack
    // the exact invariants the decode/verify path promises to hold.
    let rejected_cases: Vec<(String, &str)> = vec![
        (mutate("/format", Value::from("not.lk")), "wrong format tag"),
        (mutate("/version", Value::from(u64::MAX)), "hostile version"),
        (mutate("/module/entry", Value::from(9_999)), "entry out of bounds"),
        (
            mutate("/module/functions/0/register_count", Value::from(0)),
            "zeroed register count (register operands go out of bounds)",
        ),
        (
            mutate("/module/functions/0/code", Value::Array(Vec::new())),
            "emptied entry code (facts/jumps reference missing instructions)",
        ),
        (
            mutate(
                "/module/functions/0/performance/registers",
                serde_json::json!([4_294_967_295_u32, []]),
            ),
            "fact-table length bomb (must hit the pre-allocation ceiling)",
        ),
        (
            "[".repeat(20_000),
            "deep-nesting JSON (recursion limit, no stack overflow)",
        ),
        (String::new(), "empty input"),
        ("null".to_string(), "JSON null"),
        ("{}".to_string(), "empty object"),
    ];
    for (payload, what) in &rejected_cases {
        assert_eq!(
            decode_without_panicking(payload, what),
            Outcome::Rejected,
            "hostile artifact must be rejected: {what}"
        );
    }

    // Corrupt instruction words land wherever `Instr::try_from_raw` or the
    // register/jump verifier classifies them — any `Err` is fine, panics are
    // not, and acceptance is impossible for all-ones bits on a tiny function.
    let corrupt_code = mutate("/module/functions/0/code/0", Value::from(u32::MAX));
    assert_eq!(
        decode_without_panicking(&corrupt_code, "all-ones instruction word"),
        Outcome::Rejected,
        "an all-ones instruction word must not verify against a tiny function"
    );
}
