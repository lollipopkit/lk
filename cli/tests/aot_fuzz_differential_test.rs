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
#![cfg(feature = "llvm")]

use std::fmt::Write as _;
use std::fs::{self, File};
use std::io::Write as _;
use std::path::PathBuf;
use std::process::Command;

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

    /// Tier 1 hybrid shape (`docs/llvm/tier1-hybrid.md`): an *eligible-but-
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
            self.vars = saved;
            self.lists = saved_lists;
            self.maps = saved_maps;
            self.fns = saved_fns;
            // A top-level `let f = |…| …` lambda is call-site identical to a
            // named `fn`, but exercises the zero-capture closure lowering
            // (MakeClosure → GlobalRef::Lambda devirtualization).
            if self.rng.chance(30) {
                let _ = writeln!(out, "let {name} = |{}| {body};", params.join(", "));
            } else {
                let _ = writeln!(out, "fn {name}({}) {{ return {body}; }}", params.join(", "));
            }
            self.fns.push(FnSig { name, arity });
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

    // MIR-gated native compile: either it lowers (fully native or Tier 1
    // hybrid — LK_AOT_HYBRID exercises the bridge on the generated hybrid
    // helpers), or it must fail with a graceful Unsupported reason (lower()
    // totality) — never a panic.
    let mut exe_cmd = Command::new(bin_path());
    exe_cmd
        .current_dir(dir)
        .args(["compile", &file])
        .env("LK_AOT_HYBRID", "1");
    let exe = output_with_timeout(exe_cmd, "native compile", &context("native compile"));
    let exe_stderr = String::from_utf8_lossy(&exe.stderr).into_owned();
    assert!(
        !exe_stderr.contains("panicked at"),
        "{}\nstderr: {exe_stderr}",
        context("AOT compile panicked (lower()/codegen must be total)")
    );
    if !exe.status.success() {
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
        return CaseOutcome { compared: false };
    }
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
    CaseOutcome { compared: true }
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
        .args(["build", "-p", "lk-api", "--features", "ffi", "--release"])
        .status()
        .expect("spawn cargo build lk-api");
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
    for case in 0..cases {
        let case_seed = seed.wrapping_add(case);
        let mut generator = Generator::new(case_seed);
        let (source, expect_hybrid) = generator.program();
        let outcome = run_case(&dir, &format!("fuzz_{case}"), &source, case_seed, expect_hybrid);
        if outcome.compared {
            compared += 1;
        }
    }

    println!("fuzz differential: {compared}/{cases} cases natively compared (seed {seed:#x})");
    let _ = fs::remove_dir_all(&dir);

    // The generator targets the MIR-lowerable subset; if almost nothing lowers
    // any more, the fuzz has silently degraded into a VM-only smoke test.
    assert!(
        compared * 4 >= cases,
        "only {compared}/{cases} generated programs lowered natively; the generator or the \
         MIR pipeline coverage has regressed"
    );
}
