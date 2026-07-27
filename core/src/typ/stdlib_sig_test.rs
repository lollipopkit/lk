use super::stdlib_sig::*;
use crate::val::Type;

#[test]
fn language_types_resolve_as_themselves() {
    assert_eq!(type_from_text("Int"), Type::Int);
    assert_eq!(type_from_text("String"), Type::String);
    assert_eq!(type_from_text("Bool"), Type::Bool);
    assert_eq!(type_from_text("Nil"), Type::Nil);
    assert_eq!(type_from_text("List<String>"), Type::List(Box::new(Type::String)));
    assert_eq!(
        type_from_text("Map<String, Int>"),
        Type::Map(Box::new(Type::String), Box::new(Type::Int))
    );
    assert_eq!(type_from_text("String?"), Type::Optional(Box::new(Type::String)));
    assert_eq!(type_from_text("Int | Float"), Type::Union(vec![Type::Int, Type::Float]));
}

#[test]
fn number_is_the_documented_spelling_of_int_or_float() {
    assert_eq!(type_from_text("Number"), Type::Union(vec![Type::Int, Type::Float]));
}

#[test]
fn opaque_runtime_handles_resolve_to_any() {
    for text in ["Bytes", "Resource", "Stream", "Cursor", "Slice", "Value", "Fn"] {
        assert_eq!(type_from_text(text), Type::Any, "{text} should not constrain a call");
    }
}

#[test]
fn an_opaque_arm_makes_the_whole_union_any() {
    // `fs.write(path: String, data: Bytes | String)` must keep accepting a
    // string. Resolving the arms independently would give `Any | String`, and
    // a union carrying `Any` is just `Any` — say so directly.
    assert_eq!(type_from_text("Bytes | String"), Type::Any);
}

#[test]
fn unknown_text_widens_to_any_rather_than_naming_a_type() {
    // `Type::Named("Frobnicate")` would make every call to the function fail:
    // no argument the checker can infer is assignable to a type it has never
    // seen declared.
    assert_eq!(type_from_text("Frobnicate"), Type::Any);
    assert_eq!(type_from_text(""), Type::Any);
}

#[test]
fn optional_of_an_opaque_type_stays_any() {
    assert_eq!(type_from_text("Resource?"), Type::Any);
}

#[test]
fn resolving_splits_positional_named_and_optional_parameters() {
    const PARAMS: &[StdlibParamSig] = &[
        StdlibParamSig {
            name: "value",
            ty: "Int",
            optional: false,
            named: false,
            has_default: false,
        },
        StdlibParamSig {
            name: "min",
            ty: "Int",
            optional: true,
            named: true,
            has_default: true,
        },
    ];
    const SIGS: &[StdlibCallableSig] = &[StdlibCallableSig {
        path: "test_only.clamp",
        params: PARAMS,
        returns: "Int",
        single_arity: true,
    }];
    register_stdlib_signatures(SIGS);

    let resolved = stdlib_signature("test_only.clamp").expect("registered signature");
    // `min` stays in `params`: the generated arity check lets it be passed
    // positionally, so leaving it out here would report too many arguments.
    assert_eq!(resolved.params.len(), 2);
    assert_eq!(resolved.params[0].ty, Type::Int);
    assert!(!resolved.params[0].optional);
    assert_eq!(resolved.required_params(), 1);
    assert_eq!(resolved.return_type, Type::Int);

    let named = resolved.named_params();
    assert_eq!(named.len(), 1);
    assert_eq!(named[0].name, "min");
    assert_eq!(named[0].ty, Type::Optional(Box::new(Type::Int)));
    assert!(named[0].has_default);
}

#[test]
fn an_overloaded_callable_declares_no_type() {
    const SIGS: &[StdlibCallableSig] = &[StdlibCallableSig {
        path: "test_only.overloaded",
        params: &[],
        returns: "Int",
        single_arity: false,
    }];
    register_stdlib_signatures(SIGS);

    assert!(
        stdlib_signature("test_only.overloaded").is_none(),
        "an overloaded callable has no single signature to hand the checker"
    );
}

#[test]
fn a_registered_signature_is_looked_up_by_path() {
    const PARAMS: &[StdlibParamSig] = &[StdlibParamSig {
        name: "text",
        ty: "String",
        optional: false,
        named: false,
        has_default: false,
    }];
    const SIGS: &[StdlibCallableSig] = &[StdlibCallableSig {
        path: "test_only.len",
        params: PARAMS,
        returns: "Int",
        single_arity: true,
    }];
    register_stdlib_signatures(SIGS);

    let resolved = stdlib_signature("test_only.len").expect("registered signature");
    assert_eq!(resolved.params.len(), 1);
    assert_eq!(resolved.params[0].ty, Type::String);
    assert_eq!(resolved.return_type, Type::Int);
    assert!(stdlib_signature("test_only.missing").is_none());
}
