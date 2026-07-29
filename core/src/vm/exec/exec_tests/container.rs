use super::*;
#[test]
fn execute_compares_int_ordering() {
    let function = Function {
        consts: ConstPool {
            ints: vec![3, 5],
            ..ConstPool::default()
        },
        code: vec![
            Instr::abx(Opcode::LoadInt, 0, 0),
            Instr::abx(Opcode::LoadInt, 1, 1),
            Instr::abc(Opcode::CmpLtInt, 2, 0, 1),
            Instr::abc(Opcode::CmpGeInt, 3, 0, 1),
            Instr::abc(Opcode::Return, 2, 2, 0),
        ],
        register_count: 4,
        param_count: 0,
        positional_param_count: 0,
        param_names: Vec::new(),
        capture_count: 0,
        ..Function::default()
    };

    let result = execute(&function).expect("execute");

    assert_eq!(result.returns, vec![RuntimeVal::Bool(true), RuntimeVal::Bool(false)]);
}

#[test]
fn execute_compares_nil_and_short_strings_on_fast_path() {
    let function = Function {
        consts: ConstPool {
            strings: vec!["ok".to_string(), "no".to_string()],
            ..ConstPool::default()
        },
        code: vec![
            Instr::abc(Opcode::LoadNil, 0, 0, 0),
            Instr::abx(Opcode::LoadString, 1, 0),
            Instr::abx(Opcode::LoadString, 2, 0),
            Instr::abx(Opcode::LoadString, 3, 1),
            Instr::abc(Opcode::CmpInt, 4, 0, 1),
            Instr::abc(Opcode::CmpNeInt, 5, 0, 1),
            Instr::abc(Opcode::CmpInt, 6, 1, 2),
            Instr::abc(Opcode::CmpNeInt, 7, 1, 3),
            Instr::abc(Opcode::Return, 4, 4, 0),
        ],
        register_count: 8,
        param_count: 0,
        positional_param_count: 0,
        param_names: Vec::new(),
        capture_count: 0,
        ..Function::default()
    };

    let result = execute(&function).expect("execute");

    assert_eq!(
        result.returns,
        vec![
            RuntimeVal::Bool(false),
            RuntimeVal::Bool(true),
            RuntimeVal::Bool(true),
            RuntimeVal::Bool(true)
        ]
    );
}

#[test]
fn execute_checks_contains_for_typed_list_map_and_string() {
    let function = Function {
        consts: ConstPool {
            ints: vec![2, 9, 1],
            strings: vec![
                "ab".to_string(),
                "z".to_string(),
                "abc".to_string(),
                "answer".to_string(),
                "1".to_string(),
            ],
            ..ConstPool::default()
        },
        code: vec![
            Instr::abx(Opcode::LoadInt, 0, 0),
            Instr::abx(Opcode::LoadInt, 1, 2),
            Instr::abx(Opcode::LoadInt, 2, 0),
            Instr::abc(Opcode::NewList, 3, 1, 2),
            Instr::abx(Opcode::LoadInt, 14, 1),
            Instr::abc(Opcode::Contains, 4, 0, 3),
            Instr::abc(Opcode::Contains, 5, 14, 3),
            Instr::abx(Opcode::LoadString, 6, 0),
            Instr::abx(Opcode::LoadString, 7, 1),
            Instr::abx(Opcode::LoadString, 8, 2),
            Instr::abc(Opcode::Contains, 9, 6, 8),
            Instr::abc(Opcode::Contains, 10, 7, 8),
            Instr::abx(Opcode::LoadString, 11, 3),
            Instr::abc(Opcode::NewMap, 12, 11, 1),
            Instr::abc(Opcode::Contains, 13, 11, 12),
            Instr::abc(Opcode::Contains, 15, 0, 8),
            Instr::abx(Opcode::LoadString, 16, 4),
            Instr::abx(Opcode::LoadInt, 17, 0),
            Instr::abc(Opcode::NewMap, 18, 16, 1),
            Instr::abc(Opcode::Contains, 19, 1, 18),
            Instr::abc(Opcode::Move, 0, 4, 0),
            Instr::abc(Opcode::Move, 1, 5, 0),
            Instr::abc(Opcode::Move, 2, 9, 0),
            Instr::abc(Opcode::Move, 3, 10, 0),
            Instr::abc(Opcode::Move, 4, 13, 0),
            Instr::abc(Opcode::Move, 5, 15, 0),
            Instr::abc(Opcode::Move, 6, 19, 0),
            Instr::abc(Opcode::Return, 0, 7, 0),
        ],
        register_count: 20,
        param_count: 0,
        positional_param_count: 0,
        param_names: Vec::new(),
        capture_count: 0,
        ..Function::default()
    };

    let result = execute(&function).expect("execute");

    assert_eq!(
        result.returns,
        vec![
            RuntimeVal::Bool(true),
            RuntimeVal::Bool(false),
            RuntimeVal::Bool(true),
            RuntimeVal::Bool(false),
            RuntimeVal::Bool(true),
            RuntimeVal::Bool(false),
            RuntimeVal::Bool(true),
        ]
    );
}

#[test]
fn execute_to_iter_reads_typed_string_int_map_backing_as_pairs() {
    let function = Function {
        consts: ConstPool {
            ints: vec![10, 20],
            strings: vec!["a".to_string(), "b".to_string()],
            ..ConstPool::default()
        },
        code: vec![
            Instr::abx(Opcode::LoadString, 0, 0),
            Instr::abx(Opcode::LoadInt, 1, 0),
            Instr::abx(Opcode::LoadString, 2, 1),
            Instr::abx(Opcode::LoadInt, 3, 1),
            Instr::abc(Opcode::NewMap, 4, 0, 2),
            Instr::abc(Opcode::ToIter, 5, 4, 0),
            Instr::abc(Opcode::Return, 4, 2, 0),
        ],
        register_count: 6,
        param_count: 0,
        positional_param_count: 0,
        param_names: Vec::new(),
        capture_count: 0,
        ..Function::default()
    };

    let result = execute(&function).expect("execute");

    let RuntimeVal::Obj(map_handle) = result.returns[0] else {
        panic!("expected returned map object");
    };
    let HeapValue::Map(TypedMap::StringInt(entries)) = result.state.heap.get(map_handle).expect("map heap object")
    else {
        panic!("expected typed string-int map backing");
    };
    assert_eq!(entries.get("a").copied(), Some(10));
    assert_eq!(entries.get("b").copied(), Some(20));

    let RuntimeVal::Obj(iter_handle) = result.returns[1] else {
        panic!("expected returned iterator list object");
    };
    let HeapValue::List(crate::val::TypedList::Mixed(pairs)) =
        result.state.heap.get(iter_handle).expect("iter heap object")
    else {
        panic!("expected mixed pair list");
    };
    assert_eq!(pairs.len(), 2);

    let RuntimeVal::Obj(first_pair_handle) = pairs[0] else {
        panic!("expected first pair object");
    };
    let HeapValue::List(crate::val::TypedList::Mixed(first_pair)) = result
        .state
        .heap
        .get(first_pair_handle)
        .expect("first pair heap object")
    else {
        panic!("expected first pair mixed list");
    };
    assert_eq!(
        first_pair,
        &vec![
            RuntimeVal::ShortStr(crate::val::ShortStr::new("a").expect("short key")),
            RuntimeVal::Int(10),
        ]
    );
}

#[test]
fn execute_compares_const_string_key_maps_across_short_and_heap_keys() {
    let mut short_key_map = fast_hash_map_new();
    short_key_map.insert(
        RuntimeMapKey::ShortStr(crate::val::ShortStr::new("a").expect("short key")),
        crate::vm::ConstRuntimeValue::Int(42),
    );
    let mut heap_key_map = fast_hash_map_new();
    heap_key_map.insert(
        RuntimeMapKey::String(alloc::sync::Arc::<str>::from("a")),
        crate::vm::ConstRuntimeValue::Int(42),
    );
    let function = Function {
        consts: ConstPool {
            heap_values: vec![ConstHeapValue::Map(short_key_map), ConstHeapValue::Map(heap_key_map)],
            ..ConstPool::default()
        },
        code: vec![
            Instr::abx(Opcode::LoadHeapConst, 0, 0),
            Instr::abx(Opcode::LoadHeapConst, 1, 1),
            Instr::abc(Opcode::CmpInt, 2, 0, 1),
            Instr::abc(Opcode::Return, 2, 1, 0),
        ],
        register_count: 3,
        param_count: 0,
        positional_param_count: 0,
        param_names: Vec::new(),
        capture_count: 0,
        ..Function::default()
    };

    let result = execute(&function).expect("execute");

    assert_eq!(result.returns, vec![RuntimeVal::Bool(true)]);
}

#[test]
fn execute_mixed_map_set_index_uses_exact_string_key_semantics() {
    let mut map = fast_hash_map_new();
    map.insert(
        RuntimeMapKey::String(alloc::sync::Arc::<str>::from("a")),
        crate::vm::ConstRuntimeValue::Int(1),
    );
    map.insert(RuntimeMapKey::Int(7), crate::vm::ConstRuntimeValue::Int(0));
    let function = Function {
        consts: ConstPool {
            ints: vec![9],
            strings: vec!["a".to_string()],
            heap_values: vec![
                ConstHeapValue::Map(map),
                ConstHeapValue::LongString(alloc::sync::Arc::<str>::from("a")),
            ],
            ..ConstPool::default()
        },
        code: vec![
            Instr::abx(Opcode::LoadHeapConst, 0, 0),
            Instr::abx(Opcode::LoadString, 1, 0),
            Instr::abx(Opcode::LoadInt, 2, 0),
            Instr::abc(Opcode::SetIndex, 0, 1, 2),
            Instr::abx(Opcode::LoadHeapConst, 3, 1),
            Instr::abc(Opcode::GetIndex, 4, 0, 3),
            Instr::abc(Opcode::Return, 4, 1, 0),
        ],
        register_count: 5,
        param_count: 0,
        positional_param_count: 0,
        param_names: Vec::new(),
        capture_count: 0,
        ..Function::default()
    };

    let result = execute(&function).expect("execute");

    assert_eq!(result.returns, vec![RuntimeVal::Int(1)]);
}

#[test]
fn execute_slices_typed_list_suffix_with_slice_from() {
    let function = Function {
        consts: ConstPool {
            ints: vec![40, 1, 2],
            ..ConstPool::default()
        },
        code: vec![
            Instr::abx(Opcode::LoadInt, 0, 0),
            Instr::abx(Opcode::LoadInt, 1, 1),
            Instr::abx(Opcode::LoadInt, 2, 2),
            Instr::abc(Opcode::NewList, 3, 0, 3),
            Instr::abx(Opcode::LoadInt, 4, 1),
            Instr::abc(Opcode::SliceFrom, 5, 3, 4),
            Instr::abc(Opcode::Len, 6, 5, 0),
            Instr::abc(Opcode::Return, 6, 1, 0),
        ],
        register_count: 7,
        param_count: 0,
        positional_param_count: 0,
        param_names: Vec::new(),
        capture_count: 0,
        ..Function::default()
    };

    let result = execute(&function).expect("execute");

    assert_eq!(result.returns, vec![RuntimeVal::Int(2)]);
}

#[test]
fn execute_builds_map_rest_without_removed_keys() {
    let function = Function {
        consts: ConstPool {
            ints: vec![40, 2, 9],
            strings: vec!["a".to_string(), "b".to_string(), "c".to_string()],
            ..ConstPool::default()
        },
        code: vec![
            Instr::abx(Opcode::LoadString, 0, 0),
            Instr::abx(Opcode::LoadInt, 1, 0),
            Instr::abx(Opcode::LoadString, 2, 1),
            Instr::abx(Opcode::LoadInt, 3, 1),
            Instr::abx(Opcode::LoadString, 4, 2),
            Instr::abx(Opcode::LoadInt, 5, 2),
            Instr::abc(Opcode::NewMap, 6, 0, 3),
            Instr::abc(Opcode::Move, 7, 6, 0),
            Instr::abc(Opcode::Move, 8, 0, 0),
            Instr::abc(Opcode::MapRest, 9, 7, 1),
            Instr::abc(Opcode::GetIndex, 10, 9, 2),
            Instr::abc(Opcode::GetIndex, 11, 9, 0),
            Instr::abc(Opcode::Return, 10, 2, 0),
        ],
        register_count: 12,
        param_count: 0,
        positional_param_count: 0,
        param_names: Vec::new(),
        capture_count: 0,
        ..Function::default()
    };

    let result = execute(&function).expect("execute");

    assert_eq!(result.returns, vec![RuntimeVal::Int(2), RuntimeVal::Nil]);
}

#[test]
fn execute_map_rest_preserves_typed_string_int_backing() {
    let function = Function {
        consts: ConstPool {
            ints: vec![40, 2],
            strings: vec!["a".to_string(), "b".to_string()],
            ..ConstPool::default()
        },
        code: vec![
            Instr::abx(Opcode::LoadString, 0, 0),
            Instr::abx(Opcode::LoadInt, 1, 0),
            Instr::abx(Opcode::LoadString, 2, 1),
            Instr::abx(Opcode::LoadInt, 3, 1),
            Instr::abc(Opcode::NewMap, 4, 0, 2),
            Instr::abc(Opcode::Move, 5, 4, 0),
            Instr::abc(Opcode::Move, 6, 0, 0),
            Instr::abc(Opcode::MapRest, 7, 5, 1),
            Instr::abc(Opcode::Return, 7, 1, 0),
        ],
        register_count: 8,
        param_count: 0,
        positional_param_count: 0,
        param_names: Vec::new(),
        capture_count: 0,
        ..Function::default()
    };

    let result = execute(&function).expect("execute");
    let RuntimeVal::Obj(handle) = result.returns[0] else {
        panic!("expected map object");
    };
    let HeapValue::Map(TypedMap::StringInt(values)) = result.state.heap.get(handle).expect("heap object") else {
        panic!("expected typed string-int map");
    };

    assert_eq!(values.len(), 1);
    assert_eq!(values.get("b"), Some(&2));
}

/// A window does not copy, so the source can shrink under it. Every reader used
/// to answer "how long is this window" differently: `len()` said 3 while
/// `println` showed two elements, `to_list()` produced `[1,2,nil]`, and `==`
/// against those two elements was false.
#[test]
fn a_window_whose_source_shrank_gives_one_answer_everywhere() {
    let result = execute_source(
        r#"
        let xs = [1, 2, 3];
        let window = xs.slice(0, 3);
        xs.pop();
        return [window.len(), window.to_list(), window.last(), window == [1, 2], window.first()];
        "#,
    )
    .expect("execute source");

    let display = crate::vm::display_runtime_value(&result.returns[0], &result.state.heap);
    assert_eq!(display, "[2,[1,2],2,true,1]");
}

/// `clear()` is in the container method table for maps, sets *and* lists, but
/// a list did not have it.
#[test]
fn list_clear_empties_in_place_and_answers_the_list() {
    let result = execute_source(
        r#"
        let xs = [1, 2, 3];
        let answered = xs.clear();
        return [xs, answered, xs.push(7)];
        "#,
    )
    .expect("execute source");

    let display = crate::vm::display_runtime_value(&result.returns[0], &result.state.heap);
    assert_eq!(display, "[[7],[7],[7]]");
}

/// `try` is an expression, like `if` and `match`. It was a statement, so
/// `let r = try { … } catch e { … };` was a syntax error and the way to get a
/// value out was to declare a `nil` first and assign into it from both halves.
#[test]
fn try_is_an_expression_and_both_halves_carry_its_value() {
    let result = execute_source(
        r#"
        fn risky(n) { return 100 % n; }
        let ok = try { risky(30) } catch e { -1 };
        let caught = try { risky(0) } catch e { -1 };
        let payload = try { risky(0) } catch e { e };
        // A half that ends in a statement has no value, as in an `if`.
        let empty = try { risky(0) } catch e { let unused = 1; };
        // The inner one is the outer's tail, so it is a value too.
        let nested = try { try { risky(0) } catch e { 2 } } catch e { 3 };
        return [ok, caught, payload, empty, nested];
        "#,
    )
    .expect("execute source");

    let display = crate::vm::display_runtime_value(&result.returns[0], &result.state.heap);
    assert_eq!(display, "[10,-1,\"modulo by zero\",nil,2]");
}

/// Statement position is unchanged — the value is discarded, as an `if` or a
/// `match` in statement position is. It compiles to the region it always did,
/// with no value register: one written *inside* a protected region has to
/// survive it, and reserving one nobody reads took `try { f(); } catch e { … }`
/// off the native path.
#[test]
fn try_in_statement_position_still_runs_for_effect() {
    let result = execute_source(
        r#"
        let log = [];
        try { let bad = 1 % 0; log.push("body"); } catch e { log.push("handler"); }
        try { log.push("fine"); } catch e { log.push("unreachable"); }
        return log;
        "#,
    )
    .expect("execute source");

    let display = crate::vm::display_runtime_value(&result.returns[0], &result.state.heap);
    assert_eq!(display, "[\"handler\",\"fine\"]");
}

/// A receiver that is a plain local *is* that local's register, not a copy.
/// Capturing the same local in a closure boxes it in place, so an argument
/// containing such a closure changed what the already-taken receiver pointed
/// at — and the call ran against the cell: `xs.map(|x| x + xs.len())` answered
/// "UpvalCell has no method 'map'".
#[test]
fn a_method_receiver_survives_an_argument_that_captures_it() {
    let result = execute_source(
        r#"
        let xs = [1, 2];
        let widened = xs.map(|x| x + xs.len());
        let kept = xs.filter(|x| xs.len() > 1);
        let boxed: List<Any> = [1];
        boxed.push(|| boxed.len());
        let m = {"a": 1};
        m.set("b", || m.len());
        return [widened, kept, boxed.len(), m.len()];
        "#,
    )
    .expect("execute source");

    let display = crate::vm::display_runtime_value(&result.returns[0], &result.state.heap);
    assert_eq!(display, "[[3,4],[1,2],2,2]");
}

/// Containers were the other half of the "both `Obj`, one rank, therefore
/// equal" hole that made sorting long strings a no-op: sorting a list of lists
/// left it exactly as it was.
#[test]
fn sorting_orders_lists_element_by_element() {
    let result = execute_source(
        r#"
        let pairs = [[1, "b"], [1, "a"], [0, "c"]];
        let lengths = [[1, 2, 3], [1, 2], [1]];
        let long = [["zzzzzzzzzz"], ["aaaaaaaaaa"]];
        let mixed: List<Any> = [{"a": 1}, [1], "s"];
        return [pairs.sort(), lengths.sort(), long.sort(), mixed.sort()];
        "#,
    )
    .expect("execute source");

    let display = crate::vm::display_runtime_value(&result.returns[0], &result.state.heap);
    assert_eq!(
        display,
        "[[[0,\"c\"],[1,\"a\"],[1,\"b\"]],[[1],[1,2],[1,2,3]],[[\"aaaaaaaaaa\"],[\"zzzzzzzzzz\"]],[\"s\",[1],{\"a\":1}]]"
    );
}

/// A local captured by a closure lives in a cell, and the cell lives in the
/// local's register. Compound assignment computed the new value *into that
/// register*, overwriting the cell — so the store that followed found no cell:
/// `let n = 1; let f = || n; n += 1;` raised
/// "StoreCellVal expected UpvalCell object". Plain `n = n + 1` always worked,
/// which is what made it look like an arithmetic problem.
#[test]
fn compound_assignment_to_a_captured_local_updates_its_cell() {
    let result = execute_source(
        r#"
        let n = 1;
        let read = || n;
        n += 4;
        n *= 2;
        n -= 3;
        let text = "a";
        let read_text = || text;
        text += "b";
        return [n, read(), text, read_text()];
        "#,
    )
    .expect("execute source");

    let display = crate::vm::display_runtime_value(&result.returns[0], &result.state.heap);
    assert_eq!(display, "[7,7,\"ab\",\"ab\"]");
}

/// The receiver-aliasing family, in the index-assignment position: a closure in
/// the *value* boxes the target's local, and the write then landed on the cell.
#[test]
fn an_index_assignment_target_survives_a_value_that_captures_it() {
    let result = execute_source(
        r#"
        let xs: List<Any> = [1, 2];
        xs[0] = || xs.len();
        let m: Map<String, Any> = {"a": 1};
        m["b"] = || m.len();
        return [xs.len(), m.len()];
        "#,
    )
    .expect("execute source");

    let display = crate::vm::display_runtime_value(&result.returns[0], &result.state.heap);
    assert_eq!(display, "[2,2]");
}

/// `impl Type { … }` was a syntax error, and there is no UFCS — so a struct
/// could only get a method by declaring a trait that said nothing and
/// implementing *that*. The machinery was already there: dispatch keys on the
/// target type, not on the trait.
#[test]
fn an_inherent_impl_gives_a_type_its_own_methods() {
    let result = execute_source(
        r#"
        struct Point { x: Int, y: Int }
        impl Point {
            fn norm2(self) -> Int { return self.x * self.x + self.y * self.y; }
            fn scaled(self, by: Int) -> Point { return Point { x: self.x * by, y: self.y * by }; }
        }
        trait Area { fn area(self) -> Int; }
        impl Area for Point { fn area(self) -> Int { return self.x * self.y; } }

        let p = Point { x: 3, y: 4 };
        return [p.norm2(), p.scaled(2).x, p.area()];
        "#,
    )
    .expect("execute source");

    let display = crate::vm::display_runtime_value(&result.returns[0], &result.state.heap);
    assert_eq!(display, "[25,6,12]");
}

/// An inherent impl carries no trait, so nothing is *promised* — but a trait
/// impl still has to keep its promise.
#[test]
fn a_trait_impl_still_has_to_implement_the_trait() {
    let error = execute_source(
        r#"
        trait Area { fn area(self) -> Int; }
        struct Point { x: Int }
        impl Area for Point { }
        return 1;
        "#,
    )
    .expect_err("an unimplemented trait method");
    assert!(error.to_string().contains("not implemented"), "{error}");
}

/// A trait impl carries the trait's methods and nothing else. It used to accept
/// anything, and it had to: with `impl Type { … }` a syntax error and no UFCS,
/// a trait impl was the only place a method could live.
#[test]
fn a_trait_impl_rejects_a_method_the_trait_never_declared() {
    let error = execute_source(
        r#"
        trait Area { fn area(self) -> Int; }
        struct Point { x: Int }
        impl Area for Point {
            fn area(self) -> Int { return self.x; }
            fn unrelated(self) -> Int { return 0; }
        }
        return 1;
        "#,
    )
    .expect_err("`unrelated` is not part of `Area`");
    assert!(error.to_string().contains("is not declared by trait"), "{error}");

    // …and the fix the message names actually works.
    let result = execute_source(
        r#"
        trait Area { fn area(self) -> Int; }
        struct Point { x: Int }
        impl Area for Point { fn area(self) -> Int { return self.x; } }
        impl Point { fn unrelated(self) -> Int { return 7; } }
        return [Point { x: 1 }.area(), Point { x: 1 }.unrelated()];
        "#,
    )
    .expect("execute source");
    let display = crate::vm::display_runtime_value(&result.returns[0], &result.state.heap);
    assert_eq!(display, "[1,7]");
}

/// A builtin container dispatches with its element type erased — a
/// `TypedList::Mixed` has nothing else to report — so the *checker* has to key
/// on the same thing. It keyed on the static type instead, and
/// `impl T for List` registered under `List<Any>` while a call on `[1, 2]`
/// looked up `List<Int>`: the method existed and could not be found. `String`
/// and `Map` worked only because neither takes that path.
#[test]
fn a_method_on_a_builtin_container_is_found_whatever_its_elements_are() {
    let result = execute_source(
        r#"
        impl List { fn second(self) -> Any { return self.get(1); } }
        impl Map { fn size(self) -> Int { return self.len(); } }
        impl Set { fn size(self) -> Int { return self.len(); } }
        return [[1, 2].second(), ["a", "b"].second(), {"k": 1}.size(), Set([1, 2]).size()];
        "#,
    )
    .expect("execute source");

    let display = crate::vm::display_runtime_value(&result.returns[0], &result.state.heap);
    assert_eq!(display, "[2,\"b\",1,2]");
}

/// The compiler picks a dedicated opcode for `len`/`push`/`set`/`split`/`join`
/// from the method *name* alone — it has no type for the receiver there. That
/// is right for a list and wrong for a struct with a method of that name:
/// `s.len()` answered "Len target object is not sized", and the four that take
/// arguments failed at *compile* time on arity, so the method could not even be
/// written.
#[test]
fn a_user_method_named_after_a_builtin_one_is_still_reachable() {
    let result = execute_source(
        r#"
        struct Boxed { items: List<Int> }
        impl Boxed {
            fn len(self) -> Int { return 99; }
            fn push(self) -> Int { return 1; }
            fn set(self) -> Int { return 2; }
            fn split(self) -> Int { return 3; }
            fn join(self) -> Int { return 4; }
        }
        let b = Boxed { items: [1] };
        // The builtins keep working on the types they belong to.
        let xs = [1, 2, 3];
        return [b.len(), b.push(), b.set(), b.split(), b.join(), xs.len()];
        "#,
    )
    .expect("execute source");

    let display = crate::vm::display_runtime_value(&result.returns[0], &result.state.heap);
    assert_eq!(display, "[99,1,2,3,4,3]");
}
