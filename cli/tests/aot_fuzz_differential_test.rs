//! Generative differential fuzz: seeded random well-typed LK programs drawn
//! from the MIR-lowerable subset (scalars, counted loops, direct calls,
//! `List<i64>`, `Map` with const keys, template strings) are run under the VM
//! and as MIR-compiled native executables, and observable behaviour (stdout +
//! success/failure) must match exactly.
//!
//! Programs the MIR pipeline rejects still count: the compile must fail with a
//! graceful `Unsupported` reason, never a panic — that pins the documented
//! totality of `lk_aot_lower::lower()` over arbitrary (well-formed) programs.
//!
//! Deterministic by default; scale with `LK_FUZZ_CASES`, reseed with
//! `LK_FUZZ_SEED`. Failures print the seed and full program source.
#![cfg(feature = "aot")]

use std::fmt::Write as _;
use std::fs::{self, File};
use std::io::Write as _;
use std::path::PathBuf;
use std::process::Command;

/// What a broken *workspace* looks like on `lk compile`'s stderr, as opposed to
/// a program the AOT declined to lower.
///
/// `rustc`'s own error shapes plus the two the driver prints when the staticlib
/// step fails. Matched rather than parsed: the point is only to tell "your
/// checkout does not compile" from "your program does not lower", and any of
/// these settles that.
const TOOLCHAIN_FAILURES: &[&str] = &[
    "error[E",
    "could not compile",
    "failed to build lk-api",
    "linking with `",
];

fn bin_path() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_lk"))
}

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

    fn chance(&mut self, percent: u64) -> bool {
        self.below(100) < percent
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Ty {
    I64,
    F64,
    Bool,
    Str,
}

#[derive(Clone)]
struct ListVar {
    name: String,
    len: usize,
    /// The literal elements at creation, so the equality shape (case 14) can
    /// generate genuine exact matches instead of comparing against unrelated
    /// random literals.
    items: Vec<String>,
}

struct MapVar {
    name: String,
    keys: Vec<String>,
}

struct FnSig {
    name: String,
    arity: usize,
}

struct Generator {
    rng: Rng,
    vars: Vec<(String, Ty)>,
    lists: Vec<ListVar>,
    maps: Vec<MapVar>,
    fns: Vec<FnSig>,
    next_id: usize,
}

impl Generator {
    fn new(seed: u64) -> Self {
        Self {
            rng: Rng(seed),
            vars: Vec::new(),
            lists: Vec::new(),
            maps: Vec::new(),
            fns: Vec::new(),
            next_id: 0,
        }
    }

    fn fresh(&mut self, prefix: &str) -> String {
        let id = self.next_id;
        self.next_id += 1;
        format!("{prefix}{id}")
    }

    fn vars_of(&self, ty: Ty) -> Vec<String> {
        self.vars
            .iter()
            .filter(|(_, t)| *t == ty)
            .map(|(n, _)| n.clone())
            .collect()
    }

    // ---- typed expressions ------------------------------------------------

    /// Integer expressions stay in small ranges (literals 0..=60, multiply only
    /// by literals 0..=6, depth <= 3) so wrapping overflow can never differ
    /// between the VM and native i64 arithmetic.
    fn int_expr(&mut self, depth: usize) -> String {
        let named = self.vars_of(Ty::I64);
        if depth == 0 || self.rng.chance(30) {
            if !named.is_empty() && self.rng.chance(55) {
                let pick = self.rng.below(named.len() as u64) as usize;
                return named[pick].clone();
            }
            return format!("{}", self.rng.below(61));
        }
        match self.rng.below(8) {
            0 => format!("({} + {})", self.int_expr(depth - 1), self.int_expr(depth - 1)),
            1 => format!("({} - {})", self.int_expr(depth - 1), self.int_expr(depth - 1)),
            2 => format!("({} * {})", self.int_expr(depth - 1), self.rng.below(7)),
            // `/` is Int/Int -> Float in LK, so integer division stays out of
            // integer expressions; `%` is Int -> Int.
            3 | 4 => format!("({} % {})", self.int_expr(depth - 1), 2 + self.rng.below(8)),
            5 if !self.lists.is_empty() => {
                let pick = self.rng.below(self.lists.len() as u64) as usize;
                let list = &self.lists[pick];
                let index = self.rng.below(list.len as u64);
                format!("{}[{}]", list.name, index)
            }
            6 if !self.fns.is_empty() => {
                let pick = self.rng.below(self.fns.len() as u64) as usize;
                let name = self.fns[pick].name.clone();
                let arity = self.fns[pick].arity;
                let args = (0..arity).map(|_| self.int_expr(1)).collect::<Vec<_>>().join(", ");
                format!("{name}({args})")
            }
            7 if !self.maps.is_empty() => {
                let pick = self.rng.below(self.maps.len() as u64) as usize;
                let map = &self.maps[pick];
                let key = map.keys[self.rng.below(map.keys.len() as u64) as usize].clone();
                format!("{}[\"{}\"]", map.name, key)
            }
            _ => format!("({} + {})", self.int_expr(depth - 1), self.rng.below(61)),
        }
    }

    fn float_expr(&mut self, depth: usize) -> String {
        const LITERALS: [&str; 6] = ["0.5", "1.5", "2.0", "2.25", "3.0", "4.5"];
        let named = self.vars_of(Ty::F64);
        if depth == 0 || self.rng.chance(35) {
            if !named.is_empty() && self.rng.chance(55) {
                let pick = self.rng.below(named.len() as u64) as usize;
                return named[pick].clone();
            }
            return LITERALS[self.rng.below(LITERALS.len() as u64) as usize].to_string();
        }
        match self.rng.below(4) {
            0 => format!("({} + {})", self.float_expr(depth - 1), self.float_expr(depth - 1)),
            1 => format!("({} - {})", self.float_expr(depth - 1), self.float_expr(depth - 1)),
            // Mixed int/float promotion is a pinned shape ("return 5 + 7.5;").
            2 => format!("({} + {})", self.int_expr(1), self.float_expr(depth - 1)),
            _ => format!(
                "({} / {})",
                self.float_expr(depth - 1),
                ["2.0", "4.0", "0.5"][self.rng.below(3) as usize]
            ),
        }
    }

    fn bool_expr(&mut self, depth: usize) -> String {
        let named = self.vars_of(Ty::Bool);
        if depth == 0 || self.rng.chance(25) {
            if !named.is_empty() && self.rng.chance(50) {
                let pick = self.rng.below(named.len() as u64) as usize;
                return named[pick].clone();
            }
            let op = ["<", "<=", ">", ">=", "==", "!="][self.rng.below(6) as usize];
            return format!("{} {op} {}", self.int_expr(1), self.int_expr(1));
        }
        if self.rng.chance(30) {
            format!("!({})", self.bool_expr(depth - 1))
        } else {
            let op = ["<", "<=", ">", ">=", "==", "!="][self.rng.below(6) as usize];
            format!("{} {op} {}", self.int_expr(depth - 1), self.int_expr(depth - 1))
        }
    }

    fn str_expr(&mut self, depth: usize) -> String {
        const LITERALS: [&str; 5] = ["ab", "x", "key", "lk", "zz"];
        let named = self.vars_of(Ty::Str);
        if depth == 0 || self.rng.chance(30) {
            if !named.is_empty() && self.rng.chance(50) {
                let pick = self.rng.below(named.len() as u64) as usize;
                return named[pick].clone();
            }
            return format!("\"{}\"", LITERALS[self.rng.below(LITERALS.len() as u64) as usize]);
        }
        match self.rng.below(3) {
            0 => format!("({} + {})", self.str_expr(depth - 1), self.str_expr(depth - 1)),
            1 => format!("\"v=${{{}}}\"", self.int_expr(1)),
            _ => {
                let head = LITERALS[self.rng.below(LITERALS.len() as u64) as usize];
                format!("\"{head}${{{}}}-${{{}}}\"", self.int_expr(1), self.str_expr(0))
            }
        }
    }

    fn expr_of(&mut self, ty: Ty, depth: usize) -> String {
        match ty {
            Ty::I64 => self.int_expr(depth),
            Ty::F64 => self.float_expr(depth),
            Ty::Bool => self.bool_expr(depth),
            Ty::Str => self.str_expr(depth),
        }
    }

    fn random_ty(&mut self) -> Ty {
        match self.rng.below(6) {
            0..=2 => Ty::I64,
            3 => Ty::F64,
            4 => Ty::Bool,
            _ => Ty::Str,
        }
    }

    // ---- statements --------------------------------------------------------

    fn statement(&mut self, out: &mut String, indent: &str) {
        match self.rng.below(19) {
            0 | 1 => {
                let ty = self.random_ty();
                let name = self.fresh("v");
                let expr = self.expr_of(ty, 2);
                let _ = writeln!(out, "{indent}let {name} = {expr};");
                self.vars.push((name, ty));
            }
            2 => {
                let named = self.vars_of(Ty::I64);
                if let Some(name) = named.first().cloned() {
                    let expr = self.int_expr(2);
                    let _ = writeln!(out, "{indent}{name} = {name} + {expr};");
                } else {
                    let name = self.fresh("v");
                    let _ = writeln!(out, "{indent}let {name} = {};", self.int_expr(2));
                    self.vars.push((name, Ty::I64));
                }
            }
            3 => {
                let name = self.fresh("xs");
                let len = 3 + self.rng.below(3) as usize;
                let items = (0..len).map(|_| format!("{}", self.rng.below(61))).collect::<Vec<_>>();
                let _ = writeln!(out, "{indent}let {name} = [{}];", items.join(", "));
                self.lists.push(ListVar { name, len, items });
            }
            4 => {
                let name = self.fresh("m");
                let key_count = 2 + self.rng.below(2) as usize;
                let keys: Vec<String> = (0..key_count).map(|k| format!("k{k}")).collect();
                let entries = keys
                    .iter()
                    .map(|key| format!("\"{key}\": {}", self.rng.below(61)))
                    .collect::<Vec<_>>()
                    .join(", ");
                let _ = writeln!(out, "{indent}let {name} = {{{entries}}};");
                self.maps.push(MapVar { name, keys });
            }
            5 => {
                let cond = self.bool_expr(2);
                let name = self.fresh("v");
                let then_expr = self.int_expr(1);
                let else_expr = self.int_expr(1);
                // The condition is always wrapped in one outer paren pair: a
                // bare `if (a + b) != c` would parse the leading group as the
                // whole condition.
                let _ = writeln!(
                    out,
                    "{indent}let {name} = 0;\n{indent}if ({cond}) {{ {name} = {then_expr}; }} else {{ {name} = {else_expr}; }}"
                );
                self.vars.push((name, Ty::I64));
            }
            6 | 7 => {
                // Counted while loop with a dedicated counter the body never
                // rewrites, so termination is guaranteed by construction.
                let counter = self.fresh("i");
                let acc = self.fresh("acc");
                let bound = 2 + self.rng.below(9);
                let _ = writeln!(out, "{indent}let {acc} = 0;");
                let _ = writeln!(out, "{indent}let {counter} = 0;");
                let _ = writeln!(out, "{indent}while ({counter} < {bound}) {{");
                let body_indent = format!("{indent}    ");
                let body_kind = self.rng.below(3);
                self.vars.push((counter.clone(), Ty::I64));
                match body_kind {
                    0 => {
                        let term = self.int_expr(1);
                        let _ = writeln!(out, "{body_indent}{acc} = {acc} + {term};");
                    }
                    1 if !self.lists.is_empty() => {
                        let pick = self.rng.below(self.lists.len() as u64) as usize;
                        let list_name = self.lists[pick].name.clone();
                        let term = self.int_expr(1);
                        let _ = writeln!(out, "{body_indent}{list_name}.push({term});");
                        let _ = writeln!(out, "{body_indent}{acc} = {acc} + {counter};");
                        // Pushed elements extend the list; recorded length stays
                        // at the declared prefix so generated reads remain
                        // in-bounds regardless of loop interleaving.
                    }
                    _ => {
                        let _ = writeln!(out, "{body_indent}{acc} = {acc} + ({counter} * 2);");
                    }
                }
                // Counter increment must stay the final statement.
                let _ = writeln!(out, "{body_indent}{counter} = {counter} + 1;");
                let _ = writeln!(out, "{indent}}}");
                self.vars.retain(|(name, _)| name != &counter);
                self.vars.push((acc, Ty::I64));
            }
            8 => {
                if let Some(pick) = self.lists.iter().map(|l| l.name.clone()).next() {
                    let acc = self.fresh("acc");
                    let _ = writeln!(out, "{indent}let {acc} = 0;");
                    let _ = writeln!(out, "{indent}for x in {pick} {{ {acc} = {acc} + x; }}");
                    self.vars.push((acc, Ty::I64));
                } else {
                    let ty = self.random_ty();
                    let name = self.fresh("v");
                    let expr = self.expr_of(ty, 2);
                    let _ = writeln!(out, "{indent}let {name} = {expr};");
                    self.vars.push((name, ty));
                }
            }
            9 => {
                // Range for: inclusive or exclusive, small deterministic bounds.
                let acc = self.fresh("acc");
                let lo = self.rng.below(4);
                let hi = lo + self.rng.below(8);
                let op = if self.rng.chance(50) { "..=" } else { ".." };
                let _ = writeln!(out, "{indent}let {acc} = 0;");
                let _ = writeln!(out, "{indent}for i in {lo}{op}{hi} {{ {acc} = {acc} + i; }}");
                self.vars.push((acc, Ty::I64));
            }
            10 => {
                // Dynamic-template string keys hammer the composite-key and
                // string-keyed map paths.
                let map = self.fresh("dm");
                let counter = self.fresh("i");
                let bound = 2 + self.rng.below(6);
                let modulus = 2 + self.rng.below(3);
                let _ = writeln!(out, "{indent}let {map} = {{}};");
                let _ = writeln!(out, "{indent}let {counter} = 0;");
                let _ = writeln!(out, "{indent}while ({counter} < {bound}) {{");
                let _ = writeln!(out, "{indent}    let key = \"k${{{counter} % {modulus}}}\";");
                let _ = writeln!(out, "{indent}    let prev = {map}[key];");
                let _ = writeln!(
                    out,
                    "{indent}    if (prev == nil) {{ {map}[key] = 1; }} else {{ {map}[key] = prev + 1; }}"
                );
                let _ = writeln!(out, "{indent}    {counter} = {counter} + 1;");
                let _ = writeln!(out, "{indent}}}");
                let total = self.fresh("v");
                let _ = writeln!(out, "{indent}let {total} = {map}.len();");
                self.vars.push((total, Ty::I64));
            }
            12 if self.rng.chance(40) => {
                // Two different lambda identities through the same helper —
                // exercises per-identity clone specialization. A named i64
                // sometimes upgrades one identity to a capturing closure
                // (env as hidden trailing args), with a mutation between
                // calls that both the VM cell and the native env must see.
                let helper = self.fresh("hof");
                let r1 = self.fresh("v");
                let r2 = self.fresh("v");
                let k = 1 + self.rng.below(5);
                let m = self.rng.below(9);
                let captured = self.vars_of(Ty::I64).first().cloned().filter(|_| self.rng.chance(50));
                let _ = writeln!(out, "{indent}fn {helper}(f, x) {{ return f(x) + f(x + 1); }}");
                let _ = writeln!(out, "{indent}let {r1} = {helper}(|p| p * {k}, {});", self.rng.below(12));
                match captured {
                    Some(captured) => {
                        let _ = writeln!(
                            out,
                            "{indent}let {r2} = {helper}(|p| p + {captured}, {});",
                            self.rng.below(12)
                        );
                        if self.rng.chance(50) {
                            let r3 = self.fresh("v");
                            let _ = writeln!(out, "{indent}{captured} = {captured} + {};", 1 + self.rng.below(7));
                            let _ = writeln!(
                                out,
                                "{indent}let {r3} = {helper}(|p| p + {captured}, {});",
                                self.rng.below(12)
                            );
                            self.vars.push((r3, Ty::I64));
                        }
                    }
                    None => {
                        let _ = writeln!(out, "{indent}let {r2} = {helper}(|p| p + {m}, {});", self.rng.below(12));
                    }
                }
                self.vars.push((r1, Ty::I64));
                self.vars.push((r2, Ty::I64));
            }
            12 if self.rng.chance(25) => {
                // Closure factory: the callee's single return is a closure
                // capturing its parameter — the summary path constructs the
                // ref at each call site (distinct environments, no call).
                let factory = self.fresh("mk");
                let f1 = self.fresh("f");
                let f2 = self.fresh("f");
                let r1 = self.fresh("v");
                let r2 = self.fresh("v");
                let _ = writeln!(out, "{indent}fn {factory}(n) {{ return |x| x * n + 1; }}");
                let _ = writeln!(out, "{indent}let {f1} = {factory}({});", 1 + self.rng.below(6));
                let _ = writeln!(out, "{indent}let {f2} = {factory}({});", 1 + self.rng.below(6));
                let _ = writeln!(out, "{indent}let {r1} = {f1}({});", self.rng.below(12));
                let _ = writeln!(out, "{indent}let {r2} = {f2}({});", self.rng.below(12));
                self.vars.push((r1, Ty::I64));
                self.vars.push((r2, Ty::I64));
            }
            13 if self.rng.chance(35) => {
                // Branchy helper with fresh capturing closures at two call
                // sites: the VM inlines the helper body, and the captured
                // local's cell promotion must survive the inline scope
                // restore (regression shape); the native side lowers it via
                // cross-block cell phis.
                let helper = self.fresh("pick");
                let r1 = self.fresh("v");
                let r2 = self.fresh("v");
                let threshold = 1 + self.rng.below(6);
                let named = self.vars_of(Ty::I64);
                let capture = match named.first() {
                    Some(name) => name.clone(),
                    None => {
                        let name = self.fresh("c");
                        let _ = writeln!(out, "{indent}let {name} = {};", self.rng.below(30));
                        self.vars.push((name.clone(), Ty::I64));
                        name
                    }
                };
                let _ = writeln!(
                    out,
                    "{indent}fn {helper}(f, x) {{ if x > {threshold} {{ return f(x); }} return f(0); }}"
                );
                let _ = writeln!(
                    out,
                    "{indent}let {r1} = {helper}(|p| p + {capture}, {});",
                    self.rng.below(12)
                );
                let _ = writeln!(
                    out,
                    "{indent}let {r2} = {helper}(|p| p * 2 + {capture}, {});",
                    self.rng.below(12)
                );
                self.vars.push((r1, Ty::I64));
                self.vars.push((r2, Ty::I64));
            }
            12 => {
                // Capturing closure: the environment is a shared mutable cell,
                // so a mutation between calls must be visible — including a
                // mutation inside a branch (cross-block cell state lowers via
                // virtual-slot phis).
                let lam = self.fresh("lam");
                let result = self.fresh("v");
                let named = self.vars_of(Ty::I64);
                if let Some(captured) = named.first().cloned() {
                    let _ = writeln!(out, "{indent}let {lam} = |p0| p0 * 2 + {captured};");
                    let arg = self.rng.below(20);
                    let _ = writeln!(out, "{indent}let {result} = {lam}({arg});");
                    if self.rng.chance(50) {
                        let bump = self.rng.below(9);
                        let second = self.fresh("v");
                        if self.rng.chance(40) {
                            let cond = self.bool_expr(1);
                            let _ = writeln!(out, "{indent}if ({cond}) {{ {captured} = {captured} + {bump}; }}");
                        } else {
                            let _ = writeln!(out, "{indent}{captured} = {captured} + {bump};");
                        }
                        let _ = writeln!(out, "{indent}let {second} = {lam}({arg});");
                        self.vars.push((second, Ty::I64));
                    }
                } else {
                    let _ = writeln!(out, "{indent}let {lam} = |p0| p0 * 3 + 1;");
                    let _ = writeln!(out, "{indent}let {result} = {lam}({});", self.rng.below(20));
                }
                self.vars.push((result, Ty::I64));
            }
            13 => {
                // List HOF pipeline over a compiled lambda (fn-pointer ABI on
                // the native side): always folds to an i64 via `reduce`.
                if let Some(list) = self.lists.iter().map(|l| l.name.clone()).next() {
                    let result = self.fresh("v");
                    let k = 1 + self.rng.below(4);
                    let m = 2 + self.rng.below(3);
                    let pipeline = match self.rng.below(3) {
                        0 => format!("{list}.map(|x| x * {k}).reduce(0, |a, b| a + b)"),
                        1 => format!("{list}.filter(|x| x % {m} == 0).reduce(0, |a, b| a + b)"),
                        _ => format!("{list}.filter(|x| x % {m} != 0).map(|x| x + {k}).reduce(0, |a, b| a + b)"),
                    };
                    let _ = writeln!(out, "{indent}let {result} = {pipeline};");
                    self.vars.push((result, Ty::I64));
                } else {
                    let name = self.fresh("v");
                    let _ = writeln!(out, "{indent}let {name} = {};", self.int_expr(2));
                    self.vars.push((name, Ty::I64));
                }
            }
            14 => {
                // List structural equality against a literal built from the
                // list's *creation* elements: an exact match, a truncation,
                // or a single perturbed element — printed directly (native
                // lowers via the lkrt eq helpers; `!=` half the time).
                if let Some(list) = self.lists.first().cloned() {
                    let mut items = list.items.clone();
                    match self.rng.below(3) {
                        0 => {}
                        1 => {
                            items.pop();
                        }
                        _ => {
                            if !items.is_empty() {
                                let idx = self.rng.below(items.len() as u64) as usize;
                                items[idx] = format!("{}", self.rng.below(61));
                            }
                        }
                    }
                    let op = if self.rng.chance(50) { "==" } else { "!=" };
                    let _ = writeln!(out, "{indent}println({} {op} [{}]);", list.name, items.join(", "));
                } else {
                    let name = self.fresh("xs");
                    let _ = writeln!(out, "{indent}let {name} = [4, 5, 6];");
                    let _ = writeln!(out, "{indent}println({name} == [4, 5, 6]);");
                    self.lists.push(ListVar {
                        name,
                        len: 3,
                        items: vec!["4".into(), "5".into(), "6".into()],
                    });
                }
            }
            11 => {
                // String list: push templated parts, observe via join.
                let list = self.fresh("sl");
                let counter = self.fresh("i");
                let bound = 1 + self.rng.below(4);
                let _ = writeln!(out, "{indent}let {list} = [];");
                let _ = writeln!(out, "{indent}let {counter} = 0;");
                let _ = writeln!(
                    out,
                    "{indent}while ({counter} < {bound}) {{ {list}.push(\"p${{{counter}}}\"); {counter} = {counter} + 1; }}"
                );
                let joined = self.fresh("v");
                let _ = writeln!(out, "{indent}let {joined} = {list}.join(\"-\");");
                self.vars.push((joined, Ty::Str));
            }
            15 => {
                // Mixed-constant list (boxed-dynamic on the native side):
                // display, constant indexing (negative / OOB-nil included),
                // and len all ride the Dyn carrier (plan M4.2).
                let name = self.fresh("dl");
                let i = self.rng.below(50);
                let f = format!("{}.5", self.rng.below(9));
                let s = format!("\"s{}\"", self.rng.below(9));
                let _ = writeln!(out, "{indent}let {name} = [{i}, {s}, {f}, true];");
                match self.rng.below(3) {
                    0 => {
                        let _ = writeln!(out, "{indent}println({name});");
                    }
                    1 => {
                        // -4..=4 covers in-bounds, negative and OOB (nil).
                        let idx = self.rng.below(9) as i64 - 4;
                        let _ = writeln!(out, "{indent}println({name}[{idx}]);");
                    }
                    _ => {
                        let _ = writeln!(out, "{indent}println({name}.len());");
                    }
                }
            }
            16 => {
                // Empty `[]` + mixed pushes: the guessed materialization is
                // contradicted mid-loop, exercising the fixpoint re-guess
                // (EmptyListGuessWrong) and Dyn boxing on every push.
                let name = self.fresh("ml");
                let counter = self.fresh("i");
                let bound = 2 + self.rng.below(5);
                let modulus = 2 + self.rng.below(2);
                let _ = writeln!(out, "{indent}let {name} = [];");
                let _ = writeln!(out, "{indent}let {counter} = 0;");
                let _ = writeln!(out, "{indent}while ({counter} < {bound}) {{");
                let _ = writeln!(
                    out,
                    "{indent}    if ({counter} % {modulus} == 0) {{ {name}.push({counter}); }} else {{ {name}.push(\"e${{{counter}}}\"); }}"
                );
                let _ = writeln!(out, "{indent}    {counter} = {counter} + 1;");
                let _ = writeln!(out, "{indent}}}");
                let _ = writeln!(out, "{indent}println({name});");
                let _ = writeln!(out, "{indent}println({name}.len());");
            }
            17 => {
                // Nested-result method family over an int list: chunk /
                // enumerate / zip / unique / take+chain — every result is a
                // Dyn list of lists (or a dedup) printed bare-text.
                if let Some(list) = self.lists.iter().map(|l| l.name.clone()).next() {
                    let n = 1 + self.rng.below(3);
                    let call = match self.rng.below(5) {
                        0 => format!("{list}.chunk({n})"),
                        1 => format!("{list}.enumerate()"),
                        2 => format!("{list}.zip({list})"),
                        3 => format!("{list}.unique()"),
                        _ => format!("{list}.take({n}).chain({list}.skip({n}))"),
                    };
                    let _ = writeln!(out, "{indent}println({call});");
                } else {
                    let name = self.fresh("xs");
                    let _ = writeln!(out, "{indent}let {name} = [7, 8, 9];");
                    let _ = writeln!(out, "{indent}println({name}.enumerate());");
                    self.lists.push(ListVar {
                        name,
                        len: 3,
                        items: vec!["7".into(), "8".into(), "9".into()],
                    });
                }
            }
            18 => {
                // Struct literal (MapStrDyn carrier): typed + optional
                // fields, reads feed arithmetic / display / ?? — including
                // the absent-field nil path.
                let ty_name = format!("S{}", self.fresh("t").trim_start_matches('t').to_owned());
                let inst = self.fresh("st");
                let a = self.rng.below(40);
                let with_opt = self.rng.chance(50);
                let _ = writeln!(out, "{indent}struct {ty_name} {{ a: Int, note: String?, on: Bool }}");
                if with_opt {
                    let _ = writeln!(
                        out,
                        "{indent}let {inst} = {ty_name} {{ a: {a}, note: \"n{a}\", on: true }};"
                    );
                } else {
                    let _ = writeln!(out, "{indent}let {inst} = {ty_name} {{ a: {a}, on: false }};");
                }
                let _ = writeln!(out, "{indent}println({inst}.a + {});", self.rng.below(7));
                let _ = writeln!(out, "{indent}println({inst}.note ?? \"none\");");
                let _ = writeln!(out, "{indent}println({inst}.on);");
            }
            _ => {
                let ty = self.random_ty();
                let name = self.fresh("v");
                let expr = self.expr_of(ty, 2);
                let _ = writeln!(out, "{indent}let {name} = {expr};");
                self.vars.push((name, ty));
            }
        }
    }

    /// Tier 1 hybrid shape (`docs/aot/tier1-hybrid.md`): an *eligible-but-
    /// unsupported* helper — its body prints through a *dynamic format
    /// string* (the documented println reject; try/catch used to be the
    /// ingredient until plan G lowered it natively and silently degraded the
    /// bridge coverage). The interface stays scalar; the `println` inside
    /// makes the bridge observable: output content *and* native/VM
    /// interleaving order both join the differential. The v2 return shapes
    /// (`kind`): 0 = statement position (discarded, the v1 zero-marshal
    /// path), 1 = scalar return consumed by native arithmetic, 2 = container
    /// return consumed by display + indexing, 3 = a raise caught by a native
    /// `try` (first-class value across the bridge).
    fn hybrid_helper(&mut self, out: &mut String, kind: u8) -> String {
        let name = self.fresh("hyb");
        let print = format!("let f = \"{name}={{}}\".trim(); println(f, p0);");
        match kind {
            1 => {
                let _ = writeln!(out, "fn {name}(p0) {{ {print} return p0 * 2; }}");
            }
            2 => {
                let _ = writeln!(out, "fn {name}(p0) {{ {print} return [p0, p0 + 1, \"tail-{name}\"]; }}");
            }
            3 => {
                let _ = writeln!(out, "fn {name}(p0) {{ {print} error(\"{name}-err: ${{p0}}\"); }}");
            }
            _ => {
                let _ = writeln!(out, "fn {name}(p0) {{ {print} }}");
            }
        }
        name
    }

    /// Generates one program; the flag reports whether it carries a hybrid
    /// helper (the harness then asserts the bridge actually engaged).
    fn program(&mut self) -> (String, bool) {
        let mut out = String::new();

        // Roughly half the programs carry a hybrid helper; its shape cycles
        // through the v1/v2 bridge surfaces (see `hybrid_helper`).
        let hybrid = if self.rng.chance(50) {
            let kind = self.rng.below(4) as u8;
            Some((self.hybrid_helper(&mut out, kind), kind))
        } else {
            None
        };

        // A container the top level declares and the helpers below mutate.
        //
        // This is the shape the generator could not produce, and it is the shape
        // that miscompiled: a `List<i64>` written to a global slot was boxed,
        // boxing re-represents a container, and the slot ended up holding a
        // *second* list while the entry went on reading the first. Both backends
        // ran, neither complained, and they printed different numbers — a
        // three-line program, found by accident while writing an example for
        // something else.
        //
        // The generator missed it because a helper's body was built with
        // `self.vars`, `self.lists` and `self.maps` emptied, so a generated
        // function could only ever touch its own parameters. Nothing it wrote
        // shared anything with the top level.
        let shared_list = if self.rng.chance(60) {
            let name = self.fresh("shared_xs");
            let _ = writeln!(out, "let {name}: List<Int> = [];");
            Some(name)
        } else {
            None
        };
        let shared_map = if self.rng.chance(40) {
            let name = self.fresh("shared_m");
            let _ = writeln!(out, "let {name}: Map<String, Int> = {{}};");
            Some(name)
        } else {
            None
        };
        // The same shape on a *different carrier*, because the carrier is what
        // was wrong last time.
        //
        // `container_ty` in the lowering decides both which globals keep their
        // own type and which are refused when a slot joins to `Dyn`, and it
        // listed the `List` and `Map` carriers only. A `Bytes` global therefore
        // got neither: `let b = "abc".bytes(); fn f(n: Int) -> Int { return
        // b[n] ?? -1; }` printed 98 interpreted and died with `runtime type
        // error` compiled, for *any* index. This generator already knew to
        // build a shared global container — it just only knew two of them, so
        // it reproduced the previous bug's carrier and not the next one's.
        //
        // `Bytes` and `Set` are immutable here (no `push` equivalent that both
        // backends lower), so the helpers *read* them; reading is what
        // miscompiled.
        let shared_bytes = if self.rng.chance(40) {
            let name = self.fresh("shared_b");
            let _ = writeln!(out, "let {name} = \"abcdef\".bytes();");
            Some(name)
        } else {
            None
        };
        let shared_set = if self.rng.chance(30) {
            let name = self.fresh("shared_s");
            let _ = writeln!(out, "let {name} = Set([1, 2, 3]);");
            Some(name)
        } else {
            None
        };

        for _ in 0..self.rng.below(3) {
            let name = self.fresh("fn_helper");
            let arity = 1 + self.rng.below(2) as usize;
            let params: Vec<String> = (0..arity).map(|p| format!("p{p}")).collect();
            // Parameters are visible only inside the helper body.
            let saved = std::mem::take(&mut self.vars);
            let saved_lists = std::mem::take(&mut self.lists);
            let saved_maps = std::mem::take(&mut self.maps);
            let saved_fns = std::mem::take(&mut self.fns);
            for param in &params {
                self.vars.push((param.clone(), Ty::I64));
            }
            let body = self.int_expr(2);
            // Some helpers reach the top level's container instead of only
            // their parameters. Written before the body's `return` so the
            // mutation happens on every call.
            let touches = match (&shared_list, &shared_map) {
                (Some(list), _) if self.rng.chance(50) => {
                    format!("{list}.push(p0); ")
                }
                (_, Some(map)) if self.rng.chance(50) => {
                    // The key interpolates rather than concatenates. `"k" + p0`
                    // retypes an unannotated `p0` as a String — string
                    // concatenation is what `+` means once one side is one — and
                    // the helper then *returns* a String while everything
                    // generated around it expects an Int. That is a program the
                    // type checker rightly refuses, and it took a 1500-case run
                    // on a fresh seed to produce one.
                    format!("{map}[\"k${{p0}}\"] = p0; ")
                }
                _ => String::new(),
            };
            // A read of a container global, folded into the returned value so a
            // wrong answer shows up in stdout rather than only in a crash. The
            // index is bounded by the helper's own parameter, which is how a
            // *runtime* index (not a constant) reaches the carrier — the
            // constant case lowered correctly even while this one did not.
            let body = match (&shared_bytes, &shared_set) {
                (Some(bytes), _) if self.rng.chance(50) => {
                    format!("({body}) + ({bytes}[p0 % 6] ?? 0)")
                }
                (_, Some(set)) if self.rng.chance(50) => {
                    format!("({body}) + (if {set}.contains(p0 % 4) {{ 1 }} else {{ 0 }})")
                }
                _ => body,
            };
            self.vars = saved;
            self.lists = saved_lists;
            self.maps = saved_maps;
            self.fns = saved_fns;
            // A top-level `let f = |…| …` lambda is call-site identical to a
            // named `fn`, but exercises the zero-capture closure lowering
            // (MakeClosure → GlobalRef::Lambda devirtualization).
            if touches.is_empty() && self.rng.chance(30) {
                let _ = writeln!(out, "let {name} = |{}| {body};", params.join(", "));
            } else {
                let _ = writeln!(out, "fn {name}({}) {{ {touches}return {body}; }}", params.join(", "));
            }
            self.fns.push(FnSig { name, arity });
        }

        // Containers that cross a call boundary, in both directions.
        //
        // The generator could not produce these either: a helper's parameters
        // were always `Int`, so a container never travelled into a function and
        // never came back out of one. That is the same question the bug found by
        // accident was about — whether the two sides are looking at one
        // container or at a copy — asked at the other boundary.
        //
        // Each of these is read *after* the call, because a program that only
        // passes a container agrees whichever answer is right.
        if self.rng.chance(50) {
            let taker = self.fresh("fn_taker");
            let _ = writeln!(
                out,
                "fn {taker}(xs: List<Int>, p0: Int) -> Int {{ xs.push(p0); return xs.len(); }}"
            );
            let arg = self.fresh("tl");
            let _ = writeln!(out, "let {arg}: List<Int> = [{}];", self.rng.below(40));
            let value = self.rng.below(50);
            // Two *siblings* that hand the same container on, not one relay.
            //
            // That plurality is the shape, and it took bisecting a real
            // miscompile to find out. One caller passing a container down does
            // not reproduce it; two callers of the same mutator, both reached
            // from the top level, do — the container's type is settled from
            // whichever call the fixpoint looked at first, and a pass that fails
            // to look leaves the other holding a guess.
            //
            // The bug this reconstructs was mine: an attempt to make the entry
            // refuse a call whose callee's return type was not yet known, which
            // recovered one shape and made this one print an empty list with no
            // fallback and no warning. See the note in `inst/global.rs`.
            let relay_a = self.fresh("fn_relay");
            let relay_b = self.fresh("fn_relay");
            let _ = writeln!(
                out,
                "fn {relay_a}(xs: List<Int>, which: Int) -> Int {{ if (which == 1) {{ {taker}(xs, 91); return 0 - 1; }} {taker}(xs, 92); return 33; }}"
            );
            let _ = writeln!(
                out,
                "fn {relay_b}(xs: List<Int>) -> Int {{ let r = {taker}(xs, 3); {taker}(xs, 4); return r; }}"
            );
            let _ = writeln!(out, "println({relay_a}({arg}, 1));");
            let _ = writeln!(out, "println({relay_b}({arg}));");
            let _ = writeln!(out, "println({taker}({arg}, {value}));");
            let _ = writeln!(out, "println({arg}.len());");
            let _ = writeln!(out, "println({arg}[{arg}.len() - 1]);");
        }
        if self.rng.chance(40) {
            let maker = self.fresh("fn_maker");
            let _ = writeln!(
                out,
                "fn {maker}(p0: Int) -> List<Int> {{ let out: List<Int> = []; out.push(p0); out.push(p0 + 1); return out; }}"
            );
            let made = self.fresh("ml");
            let seed = self.rng.below(30);
            let _ = writeln!(out, "let {made} = {maker}({seed});");
            let _ = writeln!(out, "println({made}.len());");
            let _ = writeln!(out, "println({made}[1]);");
            // And mutate what came back, which is where a returned handle that
            // was really a copy of an already-freed thing would show.
            let _ = writeln!(out, "{made}.push(7);");
            let _ = writeln!(out, "println({made}.len());");
        }
        // `defer` runs on the way out, whichever way. A generated feature with
        // no generated coverage is how the next silent difference gets in.
        if self.rng.chance(40) {
            let deferred = self.fresh("fn_deferred");
            let _ = writeln!(
                out,
                "fn {deferred}(xs: List<Int>, p0: Int) -> Int {{\n    defer xs.push(0 - 1);\n    if (p0 % 2 == 0) {{ return p0; }}\n    return p0 * 2;\n}}"
            );
            let arg = self.fresh("dl");
            let _ = writeln!(out, "let {arg}: List<Int> = [];");
            // Both branches, so the release has to happen on both.
            let _ = writeln!(out, "println({deferred}({arg}, {}));", self.rng.below(20) * 2);
            let _ = writeln!(out, "println({deferred}({arg}, {}));", self.rng.below(20) * 2 + 1);
            let _ = writeln!(out, "println({arg}.len());");
        }
        // A `try` region with something in it other than a bare call: a
        // *nested* region, and a closure built outside the region and called
        // inside it. Both are outlined into functions of their own, so what
        // crosses the boundary — a write from two frames in, a captured value
        // that has no machine word — is decided by machinery no flat
        // `try { f(); } catch` exercises. Both shapes shipped a silent wrong
        // answer that every other gate passed.
        if self.rng.chance(45) {
            let probe = self.fresh("fn_tryshape");
            let cap = self.rng.below(9) + 1;
            let bump = self.rng.below(5) + 1;
            let _ = writeln!(
                out,
                "fn {probe}(p0: Int) -> Int {{\n                     let cap = {cap};\n                     let scaled = || -> Int {{ return p0 * cap; }};\n                     let plain = || -> Int {{ return {bump}; }};\n                     let out = 0;\n                     try {{\n                         try {{\n                             if (p0 % 3 == 0) {{ error(\"inner\"); }}\n                             out = scaled() + plain();\n                         }} catch e {{ out = 0 - 1; }}\n                         if (p0 % 5 == 0) {{ error(\"outer\"); }}\n                         out = out + plain();\n                     }} catch e {{ out = out - 100; }}\n                     return out;\n}}"
            );
            // Every combination of the two raise conditions, so neither edge
            // of either region is left untaken.
            for arg in [1u64, 3, 5, 15] {
                let _ = writeln!(out, "println({probe}({arg}));");
            }
        }
        // A `try` with an **empty** handler, and a body that returns on one
        // path only.
        //
        // Every other generated `catch` has a statement in it, and that is what
        // hid this: the compiler emits no jump over an empty handler, because
        // there is nothing to jump over — so the region's fallthrough *is* its
        // handler, which is also what "the body returns on every path" looks
        // like. The lowering read the second from the first, skipped the
        // did-it-return test, and returned a value nobody parked.
        if self.rng.chance(35) {
            let probe = self.fresh("fn_emptycatch");
            let at = self.rng.below(4);
            let _ = writeln!(
                out,
                "fn {probe}(p0: Int) -> Int {{\n    let acc = 0;\n    for v in 0..4 {{\n        try {{\n            if (v == {at} && p0 > 0) {{ return v * 100; }}\n            acc = acc + v;\n        }} catch e {{ }}\n    }}\n    return acc;\n}}"
            );
            // Both the path that returns out of the region and the one that
            // does not — the second is the one that was wrong.
            for arg in [0u64, 1] {
                let _ = writeln!(out, "println({probe}({arg}));");
            }
        }
        // A `try` whose body leaves through a jump that belongs to the loop
        // *outside* it. Natively the body is a function of its own, so a `break`
        // written there has no loop to leave: it reports which way it left
        // through a flag the caller dispatches on. Three exits — `break`,
        // `continue`, `return` — plus the ordinary fall-through and a raise, so
        // every arm of that dispatch is taken.
        //
        // The loop kind is drawn because `continue` does not land in the same
        // place in each: a `for` range jumps forward to the latch, a `while`
        // jumps backward to the condition, and the first version of this got the
        // second one wrong.
        if self.rng.chance(45) {
            let probe = self.fresh("fn_tryescape");
            let brk = self.rng.below(4) + 4;
            let skip = self.rng.below(3) + 1;
            let bail = self.rng.below(3) + 5;
            let header = match self.rng.below(2) {
                0 => "for v in 0..9 {".to_string(),
                _ => "let v = 0 - 1;\n    while v < 8 {\n        v = v + 1;".to_string(),
            };
            let _ = writeln!(
                out,
                "fn {probe}(p0: Int) -> Int {{\n    let acc = 0;\n    {header}\n        try {{\n            acc = acc + v;\n            if (v == {skip}) {{ continue; }}\n            if (v == 7) {{ error(\"raised\"); }}\n            if (v == {brk}) {{ break; }}\n            if (v == {bail} && p0 > 0) {{ return acc * 10; }}\n            acc = acc + 1;\n        }} catch e {{\n            acc = acc + 100;\n        }}\n    }}\n    return acc;\n}}"
            );
            for arg in [0u64, 1] {
                let _ = writeln!(out, "println({probe}({arg}));");
            }
        }

        // A closure used as a *value* — in a list, pushed, iterated and called
        // back. Everything else the generator makes of a lambda is built and
        // called where it stands, which is the case the compiler answers
        // statically; this is the one that has to go through the runtime.
        if self.rng.chance(40) {
            let ops = self.fresh("cv");
            let a = self.rng.below(9) + 1;
            let b = self.rng.below(9) + 1;
            let _ = writeln!(out, "let {ops} = [|x| x + {a}, |x| x * {b}];");
            let _ = writeln!(out, "println({ops}[0]({a}));");
            let _ = writeln!(out, "println({ops}[1]({b}));");
            let built = self.fresh("cb");
            let _ = writeln!(out, "let {built} = [];");
            let _ = writeln!(out, "{built}.push(|x| x - {a});");
            let _ = writeln!(out, "println({built}.len());");
            let _ = writeln!(out, "println(typeof({built}[0]));");
            let sum = self.fresh("cs");
            let _ = writeln!(out, "let {sum} = 0;");
            let _ = writeln!(out, "for f in {ops} {{ {sum} = {sum} + f({b}); }}");
            let _ = writeln!(out, "println({sum});");
            // A closure capturing *another closure*: its environment is all
            // static references, which the compiler erases entirely — and a
            // value still needs one. That shipped answering "value is not
            // callable" for a function that exists.
            let base = self.fresh("cbase");
            let wrap = self.fresh("cwrap");
            let _ = writeln!(out, "let {base} = |x| x + {a};");
            let _ = writeln!(out, "let {wrap} = [|y| {base}(y) * {b}];");
            let _ = writeln!(out, "println({wrap}[0]({a}));");
        }

        let statements = 3 + self.rng.below(5);
        for _ in 0..statements {
            self.statement(&mut out, "");
        }
        if let Some((name, kind)) = &hybrid {
            let arg = self.int_expr(1);
            match kind {
                1 => {
                    let _ = writeln!(out, "let hyv = {name}({arg});\nprintln(hyv + 1);");
                }
                2 => {
                    let _ = writeln!(out, "let hyl = {name}({arg});\nprintln(hyl);\nprintln(hyl[1]);");
                }
                3 => {
                    let _ = writeln!(out, "try {{ {name}({arg}); }} catch e {{ println(\"caught: \" + e); }}");
                }
                _ => {
                    let _ = writeln!(out, "{name}({arg});");
                }
            }
        }

        // What the helpers left behind, read from the top level.
        //
        // Read *here*, after the statements have called them, because the whole
        // question is whether the top level and the functions are looking at the
        // same container. A program that only wrote it would agree either way.
        if let Some(list) = &shared_list {
            let _ = writeln!(out, "println({list}.len());");
            let _ = writeln!(out, "if ({list}.len() > 0) {{ println({list}[0]); }}");
        }
        if let Some(map) = &shared_map {
            let _ = writeln!(out, "println({map}.len());");
        }
        if let Some(bytes) = &shared_bytes {
            let _ = writeln!(out, "println({bytes}.len());");
            let _ = writeln!(out, "println({bytes}[0] ?? -1);");
            // Both ends of the range: out of range is `nil` at either one, and
            // the negative end is where the interpreter used to raise while the
            // native build answered nil.
            let _ = writeln!(out, "println({bytes}[-1] ?? -1);");
            let _ = writeln!(out, "println({bytes}[99] ?? -1);");
            let _ = writeln!(out, "println({bytes}[-99] ?? -1);");
        }
        if let Some(set) = &shared_set {
            let _ = writeln!(out, "println({set}.len());");
            let _ = writeln!(out, "println({set}.contains(2));");
        }

        // `println` lowers natively now (GetGlobal builtin + format expansion);
        // exercise several shapes: `{}` formats, plain values, extra args, and
        // randomized placeholder/argument-count mismatches (the lower-time
        // expansion must replicate `format_variadic_runtime` exactly).
        for _ in 0..self.rng.below(3) {
            if self.rng.chance(30) {
                let placeholders = self.rng.below(4) as usize;
                let args = self.rng.below(4) as usize;
                let mut fmt = String::new();
                for i in 0..placeholders {
                    if i > 0 || self.rng.chance(60) {
                        fmt.push_str(["x", "-", " ", "="][self.rng.below(4) as usize]);
                    }
                    fmt.push_str("{}");
                }
                if self.rng.chance(50) {
                    fmt.push('!');
                }
                let arg_list: Vec<String> = (0..args).map(|_| self.int_expr(1)).collect();
                if arg_list.is_empty() {
                    let _ = writeln!(out, "println(\"{fmt}\");");
                } else {
                    let _ = writeln!(out, "println(\"{fmt}\", {});", arg_list.join(", "));
                }
                continue;
            }
            match self.rng.below(4) {
                0 => {
                    let expr = self.int_expr(1);
                    let _ = writeln!(out, "println(\"{{}}\", {expr});");
                }
                1 => {
                    let a = self.int_expr(1);
                    let b = self.int_expr(1);
                    let _ = writeln!(out, "println(\"a={{}} b={{}}\", {a}, {b});");
                }
                2 => {
                    let ty = self.random_ty();
                    let expr = self.expr_of(ty, 1);
                    let _ = writeln!(out, "println({expr});");
                }
                _ => {
                    let expr = self.int_expr(1);
                    let _ = writeln!(out, "println(\"v:\", {expr}, {});", self.rng.below(61));
                }
            }
        }

        // Externalize the whole live scalar state through one interpolated
        // return template (interpolation of int/float/bool/str is a pinned
        // MIR shape), so the differential compares every variable rather
        // than a single value.
        let observed: Vec<String> = self.vars.iter().map(|(name, _)| name.clone()).take(8).collect();
        if observed.is_empty() {
            let ret_ty = self.random_ty();
            let _ = writeln!(out, "return {};", self.expr_of(ret_ty, 2));
        } else {
            let template = observed
                .iter()
                .map(|name| format!("${{{name}}}"))
                .collect::<Vec<_>>()
                .join("|");
            let _ = writeln!(out, "return \"{template}\";");
        }
        (out, hybrid.is_some())
    }
}

// ---- harness ---------------------------------------------------------------

struct CaseOutcome {
    compared: bool,
    /// The program compiled *fully native* — neither bridged nor dropped to the
    /// Tier 0 VM bundle.
    ///
    /// Counted separately from `compared` because a fallback still compiles,
    /// still runs, and still answers correctly: a lowering regression is
    /// invisible to a differential comparison by construction. `compared` alone
    /// would stay at its floor while every generated program ran on the VM.
    fully_native: bool,
}

/// Runs a command to completion with a hard timeout, killing the child on
/// expiry — a miscompiled native binary (or a future generator extension)
/// must fail the test instead of hanging CI.
fn output_with_timeout(mut command: Command, what: &str, context: &str) -> std::process::Output {
    use std::time::{Duration, Instant};
    const RUN_TIMEOUT: Duration = Duration::from_secs(60);
    let mut child = command
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap_or_else(|err| panic!("spawn {what}: {err}"));
    let started = Instant::now();
    loop {
        match child.try_wait().expect("poll child") {
            Some(_) => break,
            None if started.elapsed() > RUN_TIMEOUT => {
                let _ = child.kill();
                let _ = child.wait();
                panic!("{context}\n{what} timed out after {RUN_TIMEOUT:?}");
            }
            None => std::thread::sleep(Duration::from_millis(20)),
        }
    }
    child
        .wait_with_output()
        .unwrap_or_else(|err| panic!("collect {what}: {err}"))
}

fn run_case(dir: &std::path::Path, name: &str, source: &str, seed: u64, expect_hybrid: bool) -> CaseOutcome {
    let file = format!("{name}.lk");
    let mut f = File::create(dir.join(&file)).expect("create case file");
    f.write_all(source.as_bytes()).expect("write case file");

    let context = |stage: &str| format!("[seed {seed} / {name}] {stage}\n--- program ---\n{source}\n---");

    // VM reference run. Generated programs are well-typed, in-bounds, and
    // terminate by construction, so the VM must accept and run them.
    let mut vm_cmd = Command::new(bin_path());
    vm_cmd.current_dir(dir).arg(&file).env("LK_FORCE_VM", "1");
    let vm = output_with_timeout(vm_cmd, "VM run", &context("VM run"));
    let vm_stderr = String::from_utf8_lossy(&vm.stderr).into_owned();
    assert!(
        !vm_stderr.contains("panicked at"),
        "{}\nstderr: {vm_stderr}",
        context("VM panicked on a generated program")
    );
    assert!(
        vm.status.success(),
        "{}\nstderr: {vm_stderr}",
        context("VM rejected a generated program")
    );

    // MIR-gated native compile under the *real default* (hybrid on): either
    // it lowers (fully native or Tier 1 hybrid — the generated hybrid
    // helpers exercise the bridge), or it must fail with a graceful
    // Unsupported reason (lower() totality) — never a panic.
    let mut exe_cmd = Command::new(bin_path());
    exe_cmd.current_dir(dir).args(["compile", &file]);
    let exe = output_with_timeout(exe_cmd, "native compile", &context("native compile"));
    let exe_stderr = String::from_utf8_lossy(&exe.stderr).into_owned();
    assert!(
        !exe_stderr.contains("panicked at"),
        "{}\nstderr: {exe_stderr}",
        context("AOT compile panicked (lower()/codegen must be total)")
    );
    if !exe.status.success() {
        // A *toolchain* failure is not a compiler answer, and reading it as one
        // sends the reader to the generated program.
        //
        // The prebuild above catches a workspace that was already broken when
        // the run started. What it cannot catch is one that breaks *during* it:
        // `lk compile` rebuilds the `lk-api` staticlib on the way, so an edit
        // landing in another crate mid-run arrives here as "the AOT rejected
        // your program ungracefully", with `error[E0425]` buried in `stderr`
        // under a thousand lines of generated program. That happened, and cost
        // two rounds of reading the program instead of the checkout.
        assert!(
            !TOOLCHAIN_FAILURES.iter().any(|marker| exe_stderr.contains(marker)),
            "the toolchain itself did not build, so this says nothing about the generated \
             program. Fix the workspace and re-run.\nstderr: {exe_stderr}"
        );
        assert!(
            exe_stderr.contains("does not support"),
            "{}\nstderr: {exe_stderr}",
            context("AOT compile failed without a graceful Unsupported reason")
        );
        let reason = exe_stderr
            .lines()
            .find(|line| line.contains("MIR lowering:"))
            .and_then(|line| line.split("MIR lowering:").nth(1))
            .unwrap_or("unknown")
            .trim()
            .to_string();
        println!("  unsupported [{name}]: {reason}");
        return CaseOutcome {
            compared: false,
            fully_native: false,
        };
    }
    let fully_native = !exe_stderr.contains("Tier 1 hybrid") && !exe_stderr.contains("falling back");
    // A program with a hybrid helper either bridges it ("Tier 1 hybrid") or
    // falls back whole to Tier 0 for some *other* ineligible shape ("falling
    // back") — but it must never compile fully native: that means the
    // helper's documented-unlowerable ingredient became lowerable and the
    // bridge coverage silently degraded (it happened once, with try/catch).
    if expect_hybrid {
        assert!(
            exe_stderr.contains("Tier 1 hybrid") || exe_stderr.contains("falling back"),
            "{}\nstderr: {exe_stderr}",
            context("hybrid helper compiled fully native — bridge coverage degraded")
        );
    }

    // detect_leaks=0: raises longjmp over Rust frames whose temporaries leak
    // by design (lkrt arena model) — LSan would fail the run and swallow
    // buffered stdout. ASan memory-error checks stay on.
    let mut native_cmd = Command::new(dir.join(name));
    native_cmd.env("ASAN_OPTIONS", "detect_leaks=0");
    let native = output_with_timeout(native_cmd, "native run", &context("native run"));

    let vm_stdout = String::from_utf8_lossy(&vm.stdout);
    let native_stdout = String::from_utf8_lossy(&native.stdout);
    assert_eq!(
        vm_stdout,
        native_stdout,
        "{}\nvm status: {:?}\nnative status: {:?}\nvm stderr: {vm_stderr}\nnative stderr: {}",
        context("stdout diverged between VM and native"),
        vm.status,
        native.status,
        String::from_utf8_lossy(&native.stderr)
    );
    assert_eq!(
        vm.status.success(),
        native.status.success(),
        "{}\nvm status: {:?}\nnative status: {:?}\nvm stderr: {vm_stderr}\nnative stderr: {}",
        context("success/failure diverged between VM and native"),
        vm.status,
        native.status,
        String::from_utf8_lossy(&native.stderr)
    );
    CaseOutcome {
        compared: true,
        fully_native,
    }
}

/// `lk compile` builds the lk-api staticlib on demand *inside the compile
/// child* (`ensure_lk_api_staticlib`, cli/src/main.rs) whenever a case needs
/// the Tier 0 bundle or the Tier 1 hybrid link. On a cold cache that is a
/// full release build — minutes, not seconds — and it lands inside a single
/// case's 60s compile timeout (the nightly fresh-seed job died on exactly
/// this: the first bridge-needing case's number drifts with the seed). Warm
/// the build once, untimed, so the per-case timeout measures the compile
/// itself. Must mirror the cargo invocation in `ensure_lk_api_staticlib`.
fn warm_lk_api_staticlib() {
    let workspace = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let status = Command::new("cargo")
        .current_dir(&workspace)
        .args(["build", "-p", "lk-api-cabi", "--release"])
        .status()
        .expect("spawn cargo build lk-api-cabi");
    assert!(status.success(), "failed to prebuild the lk-api staticlib");
}

#[test]
fn fuzz_differential_vm_vs_native() {
    let cases: u64 = std::env::var("LK_FUZZ_CASES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(40);
    let seed: u64 = std::env::var("LK_FUZZ_SEED")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0xC0FF_EE00);
    warm_lk_api_staticlib();

    let dir = std::env::temp_dir().join(format!("lk_aot_fuzz_{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("create tmp dir");

    let mut compared = 0_u64;
    let mut fully_native = 0_u64;
    for case in 0..cases {
        let case_seed = seed.wrapping_add(case);
        let mut generator = Generator::new(case_seed);
        let (source, expect_hybrid) = generator.program();
        let name = format!("fuzz_{case}");
        let outcome = run_case(&dir, &name, &source, case_seed, expect_hybrid);
        if outcome.compared {
            compared += 1;
        }
        if outcome.fully_native {
            fully_native += 1;
        }
        // Drop this case's artifacts before generating the next one. Keeping
        // them all until the end costs ~30 MB per case under a sanitizer, so a
        // default 800-case run filled a 24 GB tmpfs and then failed with a
        // *link* error that looks like a lowering bug.
        let _ = fs::remove_file(dir.join(format!("{name}.lk")));
        let _ = fs::remove_file(dir.join(&name));
        let _ = fs::remove_file(dir.join(format!("{name}.lkm")));
    }

    println!(
        "fuzz differential: {compared}/{cases} cases compared, {fully_native} of them fully native (seed {seed:#x})"
    );
    let _ = fs::remove_dir_all(&dir);

    // The generator targets the MIR-lowerable subset; if almost nothing compiles
    // any more, the fuzz has silently degraded into a VM-only smoke test.
    assert!(
        compared * 4 >= cases,
        "only {compared}/{cases} generated programs compiled; the generator or the \
         MIR pipeline coverage has regressed"
    );
    // And a second floor on the number that lowered *fully native*. A program
    // that drops to the hybrid bridge or the Tier 0 bundle still compiles, still
    // runs, and still agrees with the VM — so the comparison above cannot see a
    // lowering regression at all, and the count above would not move if every
    // generated program started running on the VM. The floor is a fifth,
    // deliberately far below what is measured: the generator emits
    // deliberately-unlowerable hybrid helpers, so the real ratio is a property
    // of the generator rather than a gate, and it moves with the seed (13–19 of
    // 40 over six seeds when this was written). What the floor catches is a
    // collapse, which goes to nearly zero rather than drifting.
    assert!(
        fully_native * 5 >= compared,
        "only {fully_native}/{compared} compiled programs lowered fully native; native coverage \
         has regressed behind a fallback that still answers correctly"
    );
}
