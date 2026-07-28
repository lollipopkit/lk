//! The signatures the standard library hands to the type checker.
//!
//! `core` cannot depend on the standard library — the dependency runs the other
//! way — so what crosses the boundary is the declaration text that
//! `#[stdlib_export(params(...), returns = ...)]` already spells, not a
//! [`Type`]: those tables are `const`, and `Type` owns `Box`/`String`. Each text
//! becomes a `Type` once, the first time a signature is looked up.
//!
//! The alternative was the table this replaces: a hand-written `match` in the
//! type checker covering three modules out of twenty-three, with `math.abs` and
//! `env.get` typed `Any` because keeping a second copy accurate by hand is work
//! nobody signed up for. Generating both from the one declaration is what makes
//! the coverage total and the drift impossible.

#[cfg(not(feature = "std"))]
use crate::compat::prelude::*;
use crate::{
    compat::{once::OnceLock, sync::Mutex},
    val::{FunctionNamedParamType, Type},
};
use hashbrown::HashMap;

/// One parameter of a stdlib callable, as declared.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StdlibParamSig {
    pub name: &'static str,
    /// LK type source text — `String`, `Int?`, `List<String>`, `Int | Float`.
    pub ty: &'static str,
    /// Declared `name?: T`: may be omitted at the call site.
    pub optional: bool,
    /// Declared inside `named(...)`: passed by name rather than by position.
    pub named: bool,
    pub has_default: bool,
}

/// One stdlib callable's declared signature.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StdlibCallableSig {
    /// Dotted path, as the language spells it: `string.trim`, `encoding.json.parse`.
    pub path: &'static str,
    pub params: &'static [StdlibParamSig],
    /// LK type source text for the return type.
    pub returns: &'static str,
    /// False when the export declares more than one parameter list.
    ///
    /// An overloaded callable has no single type, and guessing one of its arms
    /// would reject valid calls to the others — so the checker is told nothing
    /// and falls back to inference, which is what it did before this table
    /// existed.
    pub single_arity: bool,
}

/// One resolved parameter.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedStdlibParam {
    pub name: String,
    pub ty: Type,
    pub optional: bool,
    pub named: bool,
    pub has_default: bool,
}

/// A callable's signature resolved into checker types.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedStdlibSig {
    /// Every declared parameter, in order.
    ///
    /// A `named(...)` parameter stays in this list: the generated arity check
    /// lets one be passed positionally too, so dropping it here would make
    /// `string.replace(text, pattern, with)` look like it had too many
    /// arguments.
    pub params: Vec<ResolvedStdlibParam>,
    pub return_type: Type,
}

impl ResolvedStdlibSig {
    /// How many leading arguments a call must supply.
    pub fn required_params(&self) -> usize {
        self.params.iter().filter(|param| !param.optional).count()
    }

    /// The named parameters, in the shape a function type wants.
    pub fn named_params(&self) -> Vec<FunctionNamedParamType> {
        self.params
            .iter()
            .filter(|param| param.named)
            .map(|param| FunctionNamedParamType {
                name: param.name.clone(),
                ty: if param.optional {
                    Type::Optional(Box::new(param.ty.clone()))
                } else {
                    param.ty.clone()
                },
                has_default: param.has_default,
            })
            .collect()
    }
}

static REGISTRY: OnceLock<Mutex<HashMap<&'static str, StdlibCallableSig>>> = OnceLock::new();

fn registry() -> &'static Mutex<HashMap<&'static str, StdlibCallableSig>> {
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Record what a stdlib module declares. Registering the same path twice with
/// the same signature is fine — module registration is idempotent and happens
/// per `ModuleRegistry`, not once per process.
pub fn register_stdlib_signatures(signatures: &'static [StdlibCallableSig]) {
    let Ok(mut registry) = registry().lock() else {
        return;
    };
    for signature in signatures {
        registry.insert(signature.path, *signature);
    }
}

/// The declared signature for a dotted path, resolved into checker types.
pub fn stdlib_signature(path: &str) -> Option<ResolvedStdlibSig> {
    let declared = {
        let registry = registry().lock().ok()?;
        *registry.get(path)?
    };
    if !declared.single_arity {
        return None;
    }
    Some(resolve(&declared))
}

/// True when any stdlib module has registered signatures.
///
/// Lets a caller tell "this path has no declared signature" apart from "no
/// standard library is linked in at all", which is the case in `core`'s own
/// tests and on bare metal.
pub fn has_stdlib_signatures() -> bool {
    registry().lock().map(|registry| !registry.is_empty()).unwrap_or(false)
}

fn resolve(declared: &StdlibCallableSig) -> ResolvedStdlibSig {
    ResolvedStdlibSig {
        params: declared
            .params
            .iter()
            .map(|param| ResolvedStdlibParam {
                name: param.name.to_string(),
                ty: type_from_text(param.ty),
                optional: param.optional,
                named: param.named,
                has_default: param.has_default,
            })
            .collect(),
        return_type: type_from_text(declared.returns),
    }
}

/// Type names the standard library documents but the language has no variant
/// for. Spelled out rather than inferred, so that adding one is a decision
/// somebody makes rather than a silent widening to `Any`.
///
/// `Number` is the only true alias — the rest name runtime handles that are
/// opaque to the type system.
const DOCUMENTED_ALIASES: &[(&str, AliasTarget)] = &[
    ("Number", AliasTarget::IntOrFloat),
    // Runtime handles. The checker cannot see *into* one, but it can tell them
    // apart from each other and from everything else, which is the part that
    // catches `bytes.slice(some_string, …)`. They were `Any` until now, so a
    // handle stopped being checked the moment it was produced.
    ("Bytes", AliasTarget::Handle),
    ("Resource", AliasTarget::Handle),
    ("Stream", AliasTarget::Handle),
    ("Cursor", AliasTarget::Handle),
    ("Slice", AliasTarget::Handle),
    // These two already have a type of their own; naming them would invent a
    // second spelling for a type the language can write.
    ("Task", AliasTarget::TaskOfAny),
    ("Channel", AliasTarget::ChannelOfAny),
    // `Value` is whatever `encoding.json.parse` decoded — genuinely any value,
    // not an opaque handle. `Fn` is a callable whose signature the declaration
    // does not state.
    ("Value", AliasTarget::Anything),
    ("Fn", AliasTarget::Anything),
];

#[derive(Clone, Copy)]
enum AliasTarget {
    IntOrFloat,
    /// A named type with no structure the checker can look inside.
    Handle,
    TaskOfAny,
    ChannelOfAny,
    Anything,
}

/// Turn one declaration's type text into a checker type.
///
/// Anything unrecognised lands on `Any`, never on `Type::Named`: a named type
/// the checker has never heard of does not merely fail to help, it makes an
/// ordinary call *fail to type-check*. `stdlib_sig_test` pins which texts take
/// this path, so a new one shows up as a test failure rather than as a silently
/// untyped function.
pub fn type_from_text(text: &str) -> Type {
    let text = text.trim();
    if text.is_empty() {
        return Type::Any;
    }

    // A union is resolved arm by arm: `Bytes | String` has an opaque arm that
    // would otherwise poison the whole type.
    if let Some(arms) = split_union(text) {
        let mut resolved = Vec::with_capacity(arms.len());
        for arm in &arms {
            let ty = type_from_text(arm);
            if ty == Type::Any {
                // One unconstrained arm makes the union unconstrained.
                return Type::Any;
            }
            resolved.push(ty);
        }
        return Type::Union(resolved);
    }

    if let Some(inner) = text.strip_suffix('?') {
        let inner = type_from_text(inner);
        return if inner == Type::Any {
            Type::Any
        } else {
            Type::Optional(Box::new(inner))
        };
    }

    if let Some((name, target)) = DOCUMENTED_ALIASES.iter().find(|(name, _)| *name == text) {
        return match target {
            AliasTarget::IntOrFloat => Type::Union(vec![Type::Int, Type::Float]),
            AliasTarget::Handle => Type::Named((*name).to_string()),
            AliasTarget::TaskOfAny => Type::Task(Box::new(Type::Any)),
            AliasTarget::ChannelOfAny => Type::Channel(Box::new(Type::Any)),
            AliasTarget::Anything => Type::Any,
        };
    }

    match Type::parse(text) {
        // `Named` here means the text is neither a language type nor a
        // documented alias — a typo in a declaration, or a type this table has
        // not been taught yet. Either way the checker must not act on it.
        Some(Type::Named(_)) | None => Type::Any,
        Some(ty) => ty,
    }
}

/// Split `A | B` at the top level, returning `None` when there is no top-level
/// `|` to split on.
fn split_union(text: &str) -> Option<Vec<&str>> {
    let mut depth = 0i32;
    let mut arms = Vec::new();
    let mut start = 0usize;
    for (idx, ch) in text.char_indices() {
        match ch {
            '<' | '[' | '(' => depth += 1,
            '>' | ']' | ')' => depth -= 1,
            '|' if depth == 0 => {
                arms.push(text[start..idx].trim());
                start = idx + 1;
            }
            _ => {}
        }
    }
    if arms.is_empty() {
        return None;
    }
    arms.push(text[start..].trim());
    Some(arms)
}
