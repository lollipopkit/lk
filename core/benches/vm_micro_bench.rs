use criterion::{Criterion, criterion_group, criterion_main};
use lkr_core::val::Val;
use std::hint::black_box;

mod vm_benches {
    use super::*;
    use lkr_core::vm::{Function, Op, Vm, VmContext};
    use std::sync::Arc;

    fn make_index_fn() -> Function {
        Function {
            consts: vec![],
            code: vec![
                Op::Index {
                    dst: 2,
                    base: 0,
                    idx: 1,
                },
                Op::Ret { base: 2, retc: 1 },
            ],
            n_regs: 3,
            protos: vec![],
            param_regs: vec![0, 1],
            named_param_regs: vec![],
            named_param_layout: Vec::new(),
            pattern_plans: Vec::new(),
            code32: None,
            bc32_decoded: None,
            analysis: None,
        }
    }

    fn make_build_map_fn(pairs: u16) -> Function {
        // Expects 2*pairs args starting at r0: k0,v0,k1,v1,...
        Function {
            consts: vec![],
            code: vec![
                Op::BuildMap {
                    dst: pairs * 2,
                    base: 0,
                    len: pairs,
                },
                Op::Ret {
                    base: pairs * 2,
                    retc: 1,
                },
            ],
            n_regs: pairs * 2 + 1,
            protos: vec![],
            // Seed all inputs as params across consecutive registers
            param_regs: (0..(pairs * 2)).collect(),
            named_param_regs: vec![],
            named_param_layout: Vec::new(),
            pattern_plans: Vec::new(),
            code32: None,
            bc32_decoded: None,
            analysis: None,
        }
    }

    fn make_indexk_fn() -> Function {
        // r0: base; consts[0]: idx -> r2 = base[const]
        Function {
            consts: vec![Val::Int(1000)],
            code: vec![Op::IndexK(2, 0, 0), Op::Ret { base: 2, retc: 1 }],
            n_regs: 3,
            protos: vec![],
            param_regs: vec![0],
            named_param_regs: vec![],
            named_param_layout: Vec::new(),
            pattern_plans: Vec::new(),
            code32: None,
            bc32_decoded: None,
            analysis: None,
        }
    }

    fn make_access_fn() -> Function {
        // r0: base; r1: field -> r2 = base[field]
        Function {
            consts: vec![],
            code: vec![Op::Access(2, 0, 1), Op::Ret { base: 2, retc: 1 }],
            n_regs: 3,
            protos: vec![],
            param_regs: vec![0, 1],
            named_param_regs: vec![],
            named_param_layout: Vec::new(),
            pattern_plans: Vec::new(),
            code32: None,
            bc32_decoded: None,
            analysis: None,
        }
    }

    fn make_accessk_fn() -> Function {
        // r0: base; consts[0]: key -> r2 = base["key"]
        Function {
            consts: vec![Val::from("k5000")],
            code: vec![Op::AccessK(2, 0, 0), Op::Ret { base: 2, retc: 1 }],
            n_regs: 3,
            protos: vec![],
            param_regs: vec![0],
            named_param_regs: vec![],
            named_param_layout: Vec::new(),
            pattern_plans: Vec::new(),
            code32: None,
            bc32_decoded: None,
            analysis: None,
        }
    }

    pub fn bench_vm_index(c: &mut Criterion) {
        let f = make_index_fn();
        let mut vm = Vm::new();
        let mut env = VmContext::new();

        // ASCII base string, index near end
        let ascii_s = "a".repeat(1024);
        let ascii_args = [Val::from(ascii_s.as_str()), Val::Int(1000)];
        c.bench_function("vm_index_ascii", |b| {
            b.iter(|| {
                black_box(vm.exec_with(&f, &mut env, Some(&ascii_args)).unwrap());
            })
        });

        // Non-ASCII base string (multi-byte), same index near end
        let uni_s = "é".repeat(1024);
        let uni_args = [Val::from(uni_s.as_str()), Val::Int(1000)];
        c.bench_function("vm_index_unicode", |b| {
            b.iter(|| {
                black_box(vm.exec_with(&f, &mut env, Some(&uni_args)).unwrap());
            })
        });
    }

    pub fn bench_vm_build_map(c: &mut Criterion) {
        let pairs: u16 = 64;
        let f = make_build_map_fn(pairs);
        let mut vm = Vm::new();
        let mut env = VmContext::new();

        // Prepare 64 key/value args
        let mut args: Vec<Val> = Vec::with_capacity((pairs * 2) as usize);
        for i in 0..pairs as usize {
            args.push(Val::from(format!("k{}", i)));
            args.push(Val::Int(i as i64));
        }

        c.bench_function("vm_build_map_64", |b| {
            b.iter(|| {
                black_box(vm.exec_with(&f, &mut env, Some(&args)).unwrap());
            })
        });
    }

    pub fn bench_vm_indexk(c: &mut Criterion) {
        let f = make_indexk_fn();
        let mut vm = Vm::new();
        let mut env = VmContext::new();
        // ASCII string base
        let ascii_s = "a".repeat(1200);
        let ascii_args = [Val::from(ascii_s.as_str())];
        c.bench_function("vm_indexk_ascii", |b| {
            b.iter(|| {
                black_box(vm.exec_with(&f, &mut env, Some(&ascii_args)).unwrap());
            })
        });
    }

    pub fn bench_vm_access_vs_accessk(c: &mut Criterion) {
        // Build a large map and hit a middle key repeatedly
        let mut map = std::collections::HashMap::new();
        for i in 0..10_000 {
            map.insert(format!("k{}", i), Val::Int(i as i64));
        }
        let base = Val::from(map);
        let key = Val::from("k5000");

        let f_dyn = make_access_fn();
        let f_k = make_accessk_fn();
        let mut vm = Vm::new();
        let mut env = VmContext::new();

        // Dynamic field in register
        let dyn_args = [base.clone(), key];
        c.bench_function("vm_access_map_dynamic", |b| {
            b.iter(|| {
                black_box(vm.exec_with(&f_dyn, &mut env, Some(&dyn_args)).unwrap());
            })
        });

        // Const field in const pool
        let k_args = [base];
        c.bench_function("vm_accessk_map_const", |b| {
            b.iter(|| {
                black_box(vm.exec_with(&f_k, &mut env, Some(&k_args)).unwrap());
            })
        });
    }

    fn make_index_list_fn() -> Function {
        Function {
            consts: vec![],
            code: vec![
                Op::Index {
                    dst: 2,
                    base: 0,
                    idx: 1,
                },
                Op::Ret { base: 2, retc: 1 },
            ],
            n_regs: 3,
            protos: vec![],
            param_regs: vec![0, 1],
            named_param_regs: vec![],
            named_param_layout: Vec::new(),
            pattern_plans: Vec::new(),
            code32: None,
            bc32_decoded: None,
            analysis: None,
        }
    }

    fn make_indexk_list_fn() -> Function {
        Function {
            consts: vec![Val::Int(1024)],
            code: vec![Op::IndexK(2, 0, 0), Op::Ret { base: 2, retc: 1 }],
            n_regs: 3,
            protos: vec![],
            param_regs: vec![0],
            named_param_regs: vec![],
            named_param_layout: Vec::new(),
            pattern_plans: Vec::new(),
            code32: None,
            bc32_decoded: None,
            analysis: None,
        }
    }

    pub fn bench_vm_index_list_vs_indexk(c: &mut Criterion) {
        let list: Vec<Val> = (0..2048).map(Val::Int).collect();
        let base = Val::List(Arc::from(list));

        // Dynamic index in register
        let f_dyn = make_index_list_fn();
        let mut vm = Vm::new();
        let mut env = VmContext::new();
        let dyn_args = [base.clone(), Val::Int(1024)];
        c.bench_function("vm_index_list_dynamic", |b| {
            b.iter(|| {
                black_box(vm.exec_with(&f_dyn, &mut env, Some(&dyn_args)).unwrap());
            })
        });

        // Const index from const pool
        let f_k = make_indexk_list_fn();
        let k_args = [base];
        c.bench_function("vm_index_list_const", |b| {
            b.iter(|| {
                black_box(vm.exec_with(&f_k, &mut env, Some(&k_args)).unwrap());
            })
        });
    }

    fn make_access_obj_fn() -> Function {
        Function {
            consts: vec![],
            code: vec![Op::Access(2, 0, 1), Op::Ret { base: 2, retc: 1 }],
            n_regs: 3,
            protos: vec![],
            param_regs: vec![0, 1],
            named_param_regs: vec![],
            named_param_layout: Vec::new(),
            pattern_plans: Vec::new(),
            code32: None,
            bc32_decoded: None,
            analysis: None,
        }
    }

    fn make_accessk_obj_fn() -> Function {
        Function {
            consts: vec![Val::from("field500")],
            code: vec![Op::AccessK(2, 0, 0), Op::Ret { base: 2, retc: 1 }],
            n_regs: 3,
            protos: vec![],
            param_regs: vec![0],
            named_param_regs: vec![],
            named_param_layout: Vec::new(),
            pattern_plans: Vec::new(),
            code32: None,
            bc32_decoded: None,
            analysis: None,
        }
    }

    pub fn bench_vm_access_obj_vs_accessk(c: &mut Criterion) {
        // Build an Object with many string fields
        let mut fields = std::collections::HashMap::new();
        for i in 0..1000 {
            fields.insert(format!("field{}", i), Val::Int(i as i64));
        }
        let base = Val::object("MyType", fields);
        let key = Val::from("field500");

        let f_dyn = make_access_obj_fn();
        let f_k = make_accessk_obj_fn();
        let mut vm = Vm::new();
        let mut env = VmContext::new();

        let dyn_args = [base.clone(), key];
        c.bench_function("vm_access_obj_dynamic", |b| {
            b.iter(|| {
                black_box(vm.exec_with(&f_dyn, &mut env, Some(&dyn_args)).unwrap());
            })
        });

        let k_args = [base];
        c.bench_function("vm_accessk_obj_const", |b| {
            b.iter(|| {
                black_box(vm.exec_with(&f_k, &mut env, Some(&k_args)).unwrap());
            })
        });
    }

    pub fn bench_vm_access_map_ic(c: &mut Criterion) {
        // Build two maps with the same content but different identities
        let mut m1 = std::collections::HashMap::new();
        let mut m2 = std::collections::HashMap::new();
        for i in 0..10_000 {
            let k = format!("k{}", i);
            m1.insert(k.clone(), Val::Int(i as i64));
            m2.insert(k, Val::Int(i as i64));
        }
        let base1 = Val::from(m1);
        let base2 = Val::from(m2);
        // Two distinct Arc<str> keys with the same content
        let key1 = Val::from(String::from("k5000"));
        let key2 = Val::from(String::from("k5000"));

        let f = make_access_fn();
        let mut vm = Vm::new();
        let mut env = VmContext::new();
        // Warm IC
        let _ = vm
            .exec_with(&f, &mut env, Some(&[base1.clone(), key1.clone()]))
            .unwrap();

        // IC hit: same base + same key each iter
        let args_hit = [base1.clone(), key1.clone()];
        c.bench_function("vm_access_map_ic_hit", |b| {
            b.iter(|| {
                black_box(vm.exec_with(&f, &mut env, Some(&args_hit)).unwrap());
            })
        });

        // IC miss by base: toggle between two different map identities
        let args_b1 = [base1.clone(), key1.clone()];
        let args_b2 = [base2.clone(), key1.clone()];
        c.bench_function("vm_access_map_ic_miss_base", |b| {
            let mut t = false;
            b.iter(|| {
                t = !t;
                let args = if t { &args_b1 } else { &args_b2 };
                black_box(vm.exec_with(&f, &mut env, Some(args)).unwrap());
            })
        });

        // IC miss by key: same base, alternate distinct key pointers
        let args_k1 = [base1.clone(), key1];
        let args_k2 = [base1, key2];
        c.bench_function("vm_access_map_ic_miss_key", |b| {
            let mut t = false;
            b.iter(|| {
                t = !t;
                let args = if t { &args_k1 } else { &args_k2 };
                black_box(vm.exec_with(&f, &mut env, Some(args)).unwrap());
            })
        });
    }

    pub fn bench_vm_access_obj_ic(c: &mut Criterion) {
        // Build two Objects with identical fields but different identities
        let mut fields1 = std::collections::HashMap::new();
        let mut fields2 = std::collections::HashMap::new();
        for i in 0..1000 {
            let k = format!("field{}", i);
            fields1.insert(k.clone(), Val::Int(i as i64));
            fields2.insert(k, Val::Int(i as i64));
        }
        let base1 = Val::object("MyType", fields1);
        let base2 = Val::object("MyType", fields2);
        // Two distinct keys with same content
        let key1 = Val::from(String::from("field500"));
        let key2 = Val::from(String::from("field500"));

        let f = make_access_obj_fn();
        let mut vm = Vm::new();
        let mut env = VmContext::new();
        // Warm IC
        let _ = vm
            .exec_with(&f, &mut env, Some(&[base1.clone(), key1.clone()]))
            .unwrap();

        // IC hit: same object + same key
        let args_hit = [base1.clone(), key1.clone()];
        c.bench_function("vm_access_obj_ic_hit", |b| {
            b.iter(|| {
                black_box(vm.exec_with(&f, &mut env, Some(&args_hit)).unwrap());
            })
        });

        // IC miss by object identity
        let args_o1 = [base1.clone(), key1.clone()];
        let args_o2 = [base2.clone(), key1.clone()];
        c.bench_function("vm_access_obj_ic_miss_base", |b| {
            let mut t = false;
            b.iter(|| {
                t = !t;
                let args = if t { &args_o1 } else { &args_o2 };
                black_box(vm.exec_with(&f, &mut env, Some(args)).unwrap());
            })
        });

        // IC miss by key pointer (same content)
        let args_k1 = [base1.clone(), key1];
        let args_k2 = [base1, key2];
        c.bench_function("vm_access_obj_ic_miss_key", |b| {
            let mut t = false;
            b.iter(|| {
                t = !t;
                let args = if t { &args_k1 } else { &args_k2 };
                black_box(vm.exec_with(&f, &mut env, Some(args)).unwrap());
            })
        });
    }

    pub fn bench_vm_indexk_unicode(c: &mut Criterion) {
        let f = make_indexk_fn();
        let mut vm = Vm::new();
        let mut env = VmContext::new();
        let uni_s = "é".repeat(1200);
        let args = [Val::from(uni_s.as_str())];
        c.bench_function("vm_indexk_unicode", |b| {
            b.iter(|| {
                black_box(vm.exec_with(&f, &mut env, Some(&args)).unwrap());
            })
        });
    }
}

criterion_group!(
    vm_only,
    vm_benches::bench_vm_index,
    vm_benches::bench_vm_build_map,
    vm_benches::bench_vm_indexk,
    vm_benches::bench_vm_access_vs_accessk,
    vm_benches::bench_vm_index_list_vs_indexk,
    vm_benches::bench_vm_access_obj_vs_accessk,
    vm_benches::bench_vm_access_map_ic,
    vm_benches::bench_vm_access_obj_ic,
    vm_benches::bench_vm_indexk_unicode
);
criterion_main!(vm_only);
