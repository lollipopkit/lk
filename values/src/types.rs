use core::fmt;

use alloc::boxed::Box;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use arcstr::ArcStr;
use hashbrown::HashMap;
use serde::{Deserialize, Serialize, Serializer};

use crate::{NumericClass, NumericHierarchy};

/// An inline short string: 0–7 UTF-8 bytes, held entirely inside a
/// `LiteralVal` with no heap allocation. `Copy`, so a clone costs no atomic.
///
/// **Invariant: `data[..len]` is valid UTF-8.** Both fields are private, so
/// nothing outside this module can build one; every construction site inside it
/// either copies a `&str`'s bytes whole, is `char::encode_utf8`'s output, or
/// appends ASCII digits to a valid prefix, and `Deserialize` goes through
/// [`ShortStr::new`]. A new construction site has to keep it —
/// `every_short_str_constructor_keeps_the_utf8_invariant` checks each one.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ShortStr {
    len: u8,
    data: [u8; 7],
}

impl ShortStr {
    /// From a `str`; `None` when it is longer than 7 bytes.
    #[inline]
    pub fn new(s: &str) -> Option<Self> {
        let bytes = s.as_bytes();
        if bytes.len() > 7 {
            return None;
        }
        let mut data = [0u8; 7];
        data[..bytes.len()].copy_from_slice(bytes);
        Some(Self {
            len: bytes.len() as u8,
            data,
        })
    }

    #[inline]
    pub fn from_char(ch: char) -> Self {
        let mut data = [0u8; 7];
        let encoded = ch.encode_utf8(&mut data);
        Self {
            len: encoded.len() as u8,
            data,
        }
    }

    #[inline]
    pub fn as_str(&self) -> &str {
        // The checked `from_utf8`, although the invariant above says it cannot
        // fail: swapping in `from_utf8_unchecked` was measured and **bought
        // nothing** (min-of-9 over a two-million-iteration map-string-key plus
        // method-call workload: 0.87s vs 0.89s). The 5% that
        // `core::str::converts::from_utf8` takes in a profile is misleading —
        // the length is capped at 7 bytes, so for ASCII the check is one byte
        // scan the compiler has already flattened.
        //
        // So no `unsafe` here: trading safety for a gain that does not measure
        // is a loss. Re-run those numbers before changing it back.
        core::str::from_utf8(&self.data[..self.len as usize]).expect("ShortStr contains valid UTF-8")
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.len as usize
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Concatenate a ShortStr prefix with an i64 suffix, returning ShortStr
    /// if the result fits in 7 bytes, otherwise a heap-allocated String.
    /// Avoids intermediate String allocation for small numbers.
    #[inline]
    pub fn concat_int(self, n: i64) -> ShortStrOrStr {
        let prefix = self.as_str().as_bytes();
        let prefix_len = prefix.len();
        // Fast path for common small non-negative integers: write digits directly.
        if (0..10000).contains(&n) {
            let num_len = decimal_len_under_10000(n as u64);
            let total_len = prefix_len + num_len;
            if total_len <= 7 {
                let mut data = [0u8; 7];
                data[..prefix_len].copy_from_slice(prefix);
                write_u64_to_buf(n as u64, &mut data[prefix_len..]);
                return ShortStrOrStr::Short(ShortStr {
                    len: total_len as u8,
                    data,
                });
            }
        }
        // Fallback: format to String and try ShortStr
        let combined = format!("{}{}", self.as_str(), n);
        if let Some(short) = ShortStr::new(&combined) {
            ShortStrOrStr::Short(short)
        } else {
            ShortStrOrStr::Str(combined)
        }
    }

    /// Concatenate an i64 prefix with a ShortStr suffix, returning ShortStr
    /// if the result fits in 7 bytes, otherwise a heap-allocated String.
    #[inline]
    pub fn concat_int_prefix(n: i64, suffix: ShortStr) -> ShortStrOrStr {
        let suffix_bytes = suffix.as_str().as_bytes();
        let suffix_len = suffix_bytes.len();
        if (0..10000).contains(&n) {
            let mut data = [0u8; 7];
            let num_len = write_u64_to_buf(n as u64, &mut data[..]);
            let total_len = num_len + suffix_len;
            if total_len <= 7 {
                data[num_len..total_len].copy_from_slice(suffix_bytes);
                return ShortStrOrStr::Short(ShortStr {
                    len: total_len as u8,
                    data,
                });
            }
        }
        let combined = format!("{}{}", n, suffix.as_str());
        if let Some(short) = ShortStr::new(&combined) {
            ShortStrOrStr::Short(short)
        } else {
            ShortStrOrStr::Str(combined)
        }
    }

    /// Concatenate two ShortStr values, returning ShortStr if the result
    /// fits in 7 bytes, otherwise a heap-allocated String.
    #[inline]
    pub fn concat(self, other: ShortStr) -> ShortStrOrStr {
        let a = self.as_str().as_bytes();
        let b = other.as_str().as_bytes();
        let total_len = a.len() + b.len();
        if total_len <= 7 {
            let mut data = [0u8; 7];
            data[..a.len()].copy_from_slice(a);
            data[a.len()..total_len].copy_from_slice(b);
            ShortStrOrStr::Short(ShortStr {
                len: total_len as u8,
                data,
            })
        } else {
            ShortStrOrStr::Str(format!("{}{}", self.as_str(), other.as_str()))
        }
    }
}

/// Result of concatenating two ShortStr values or a ShortStr with an Int.
/// Avoids String allocation when the result fits in ShortStr.
pub enum ShortStrOrStr {
    Short(ShortStr),
    Str(String),
}

/// Write a u64 as decimal ASCII to buf, returning the number of bytes written.
/// Assumes buf has at least 4 bytes of space (for numbers up to 9999).
#[inline]
fn write_u64_to_buf(n: u64, buf: &mut [u8]) -> usize {
    if n < 10 {
        buf[0] = b'0' + n as u8;
        1
    } else if n < 100 {
        buf[0] = b'0' + (n / 10) as u8;
        buf[1] = b'0' + (n % 10) as u8;
        2
    } else if n < 1000 {
        buf[0] = b'0' + (n / 100) as u8;
        buf[1] = b'0' + ((n / 10) % 10) as u8;
        buf[2] = b'0' + (n % 10) as u8;
        3
    } else if n < 10000 {
        buf[0] = b'0' + (n / 1000) as u8;
        buf[1] = b'0' + ((n / 100) % 10) as u8;
        buf[2] = b'0' + ((n / 10) % 10) as u8;
        buf[3] = b'0' + (n % 10) as u8;
        4
    } else {
        // Fallback for larger numbers
        let s = n.to_string();
        let len = s.len().min(buf.len());
        buf[..len].copy_from_slice(&s.as_bytes()[..len]);
        len
    }
}

#[inline]
fn decimal_len_under_10000(n: u64) -> usize {
    if n < 10 {
        1
    } else if n < 100 {
        2
    } else if n < 1000 {
        3
    } else {
        4
    }
}

impl fmt::Debug for ShortStr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self.as_str())
    }
}

impl fmt::Display for ShortStr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl From<ShortStr> for ArcStr {
    fn from(s: ShortStr) -> ArcStr {
        ArcStr::from(s.as_str())
    }
}

impl serde::Serialize for ShortStr {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> serde::Deserialize<'de> for ShortStr {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = <String as serde::Deserialize>::deserialize(deserializer)?;
        ShortStr::new(&s).ok_or_else(|| serde::de::Error::custom("string too long for ShortStr"))
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FunctionNamedParamType {
    pub name: String,
    pub ty: Type,
    pub has_default: bool,
}

/// A machine integer: fixed width, wrapping arithmetic, no boxing.
///
/// Deliberately *not* a refinement of [`Type::Int`]. `Int` is the language's
/// general-purpose integer — 64-bit, and the only thing the bytecode VM's
/// `RuntimeVal::Int` carries. These are what driver and MMIO code needs: a
/// `u32` register write has to be exactly 32 bits wide and has to wrap rather
/// than promote. Keeping them separate means ordinary LK code is unaffected,
/// and it makes the conversions explicit at the boundary where width matters.
///
/// The lowercase spelling (`i32`, not `I32`) marks the distinction visually:
/// capitalised names are the language's own types, lowercase ones are machine
/// types, matching how C, Rust and Zig all spell them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum IntKind {
    I8,
    I16,
    I32,
    I64,
    U8,
    U16,
    U32,
    U64,
    /// Pointer-width, signed. Concrete width depends on the target.
    Isize,
    /// Pointer-width, unsigned. Concrete width depends on the target.
    Usize,
}

impl IntKind {
    /// Whether `value` fits this width.
    ///
    /// Pointer-width kinds answer `true` for anything that fits 32 bits, since
    /// the real width is the target's and the narrower target is the binding
    /// one — a literal that fits everywhere is the only one that is portably
    /// safe to accept without a cast.
    /// Whether a literal fits.
    ///
    /// A radix literal past `i64::MAX` arrives here as its unsigned value, not as
    /// the negative carrier the `i64` would show: `let bit: u64 =
    /// 0x8000000000000000;` is a perfectly good u64 — the NX bit in a page-table
    /// entry, the high half of a 64-bit BAR — and the lexer keeps it as a `u64`
    /// (`Token::UInt`) precisely so this check sees what was written.
    ///
    /// Reinterpreting a negative value as its unsigned bit pattern *here* would
    /// not have worked: `let y: u8 = -1;` reaches this function too and is
    /// rightly refused, and the two are indistinguishable once the sign is the
    /// only evidence left. That is why the distinction is made in the lexer,
    /// where the source text still says which one it was.
    pub fn accepts_literal(self, value: i128) -> bool {
        match self.range() {
            Some((lo, hi)) => value >= lo && value <= hi,
            // Pointer width, measured at the width this compiles for.
            //
            // It used to probe `u32`/`i32` — "assume the smaller, be safe" —
            // which on a 64-bit target refuses a legal value: `let a: usize =
            // 0xFFFF_FFFF_FFFF_FFFF` was rejected while the identical `u64` was
            // accepted. Nothing else in the compiler hedges this way: the
            // unsigned-operator rewrites treat `usize` as carrier-filling
            // alongside `u64`, and pointer casts are lowered as 64-bit. The
            // range check was the only place still guessing, and it guessed
            // differently from the code it guards.
            //
            // TODO(32-bit targets): a 32-bit deployment target needs this — and
            // the pointer-width cast in `lower_cast` — to follow the target
            // rather than the host. Same TODO, one decision.
            //
            // Not reachable today, and `no_32_bit_target_is_reachable_yet`
            // (lk-aot-codegen) is what says so: every 32-bit triple is refused
            // at `isa::lookup`, so there is no target on which this range check
            // is wrong. That test fails when one arrives, and names this site.
            //
            // The checker cannot answer it by threading a target through
            // either: `lk check` has none, and bytecode is target-agnostic —
            // the target only exists at `lk compile object:<triple>`. Whatever
            // the decision turns out to be, it is a decision about *where* the
            // width comes from, not just what it is.
            None => {
                let probe = if self.is_signed() { Self::I64 } else { Self::U64 };
                probe.accepts_literal(value)
            }
        }
    }

    /// Every machine-int kind.
    ///
    /// Enumerated because the names are read from outside — the editor grammars
    /// keep their own copy, and `type_name_lists_agree` checks those against
    /// this. The *names* are not repeated here: they stay in `name()`, whose
    /// `match` the compiler keeps exhaustive.
    pub const ALL: &'static [IntKind] = &[
        Self::I8,
        Self::I16,
        Self::I32,
        Self::I64,
        Self::U8,
        Self::U16,
        Self::U32,
        Self::U64,
        Self::Isize,
        Self::Usize,
    ];

    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|kind| kind.name() == name)
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::I8 => "i8",
            Self::I16 => "i16",
            Self::I32 => "i32",
            Self::I64 => "i64",
            Self::U8 => "u8",
            Self::U16 => "u16",
            Self::U32 => "u32",
            Self::U64 => "u64",
            Self::Isize => "isize",
            Self::Usize => "usize",
        }
    }

    /// Width in bits, or `None` for the pointer-width kinds, whose width is a
    /// property of the target rather than of the type.
    pub fn bits(self) -> Option<u32> {
        Some(match self {
            Self::I8 | Self::U8 => 8,
            Self::I16 | Self::U16 => 16,
            Self::I32 | Self::U32 => 32,
            Self::I64 | Self::U64 => 64,
            Self::Isize | Self::Usize => return None,
        })
    }

    pub fn is_signed(self) -> bool {
        matches!(self, Self::I8 | Self::I16 | Self::I32 | Self::I64 | Self::Isize)
    }

    /// Inclusive value range, or `None` for pointer-width kinds. Used to reject
    /// out-of-range literals at compile time.
    pub fn range(self) -> Option<(i128, i128)> {
        let bits = self.bits()?;
        Some(if self.is_signed() {
            let max = (1i128 << (bits - 1)) - 1;
            (-(1i128 << (bits - 1)), max)
        } else {
            (0, (1i128 << bits) - 1)
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Type {
    /// Primitive types
    Int,
    /// A raw pointer: `*T` (read-only) or `*mut T`.
    ///
    /// The pointer *value* is just an address, so it exists on both backends
    /// and can be built, passed and compared under the bytecode VM. What the VM
    /// has no meaning for is *dereferencing* one — there is no address space
    /// behind it — so that operation, not the type, is what fails there.
    ///
    /// Two levels of mutability rather than Rust's `*const`/`*mut` spelling:
    /// `*T` reads, `*mut T` writes. The distinction is what stops a register
    /// marked read-only from being written by accident.
    Ptr {
        pointee: Box<Type>,
        mutable: bool,
    },
    /// Fixed-width machine integer (`i32`, `u8`, `usize`, …). See [`IntKind`].
    MachineInt(IntKind),
    Float,
    String,
    Bool,
    Nil,

    /// Generic container types
    List(Box<Type>), // List<T>
    Map(Box<Type>, Box<Type>), // Map<K, V>
    Set(Box<Type>),            // Set<T>
    /// Fixed-length heterogeneous tuple: (T0, T1, ...)
    Tuple(Vec<Type>),

    /// Function type with parameters and return type
    Function {
        params: Vec<Type>,
        named_params: Vec<FunctionNamedParamType>,
        return_type: Box<Type>,
    },

    /// Concurrency types
    Task(Box<Type>),
    Channel(Box<Type>),

    /// Union types: Int | String
    Union(Vec<Type>),

    /// Optional types: Int? (sugar for Int | Nil)
    Optional(Box<Type>),

    /// Type variables for inference (prefixed with ')
    Variable(String),

    /// Custom named types
    Named(String),

    /// Generic type with parameters: List<T>, Map<K, V>
    Generic {
        name: String,
        params: Vec<Type>,
    },

    /// Boxed runtime value that preserves inner type metadata
    Boxed(Box<Type>),

    /// Any type (top type)
    Any,

    /// An element type that is not known: `List<_>`, `Map<String, _>`.
    ///
    /// Written `_`, the same "unnamed anything" it means in a pattern, and only
    /// valid inside a type's parameter list. **Nothing is assignable to it**,
    /// which is what makes a container parameterised by it readable but not
    /// writable — the read-only view falls out of the type rather than being a
    /// second rule about containers.
    ///
    /// It exists because containers are invariant (see `is_assignable_to`):
    /// without it, a signature could not say "a list of anything", and this
    /// language has no generic functions to say it with. Covariance said it
    /// instead, and covariance over a *mutable* container is unsound —
    /// `List<Int>` widened to `List<Any>` accepted a `String` through the alias
    /// and `let b: Int = a[2]` then type-checked and held one.
    Unknown,
}

/// The parameterless builtin types, with the name the language spells each.
///
/// A list rather than a `match` arm because three other places keep their own
/// copy of these names — the tree-sitter grammar, the TextMate grammar, and
/// completion's receiver table — and a copy nobody can read is a copy that
/// drifts. `type_name_lists_agree` checks the editor grammars against this.
pub const PRIMITIVE_TYPES: &[(&str, Type)] = &[
    ("Int", Type::Int),
    ("Float", Type::Float),
    ("String", Type::String),
    ("Bool", Type::Bool),
    ("Nil", Type::Nil),
    ("Any", Type::Any),
];

/// Second spellings of types that already exist.
///
/// A language that wants to be written down to machine code without ambiguity
/// needs a name that says the width — and one that does not, for the code that
/// is not about widths. `Int` and `i64` are that pair: one type, two spellings,
/// so a driver's `i64` and a front end's `Int` are the same value and pass
/// through each other's functions without a cast.
///
/// They are *aliases*, not two types that happen to convert. Two convertible
/// types would be more ambiguity, not less: the reader would have to know which
/// one a value is to know what it does.
///
/// `isize`/`usize` are deliberately not here. Pointer width is the whole reason
/// those names exist, and equating either with a fixed width is the mistake
/// this table exists to avoid.
pub const TYPE_SPELLINGS: &[(&str, Type)] = &[("i64", Type::Int), ("f64", Type::Float)];

/// `Number` — `Int | Float`, and the one spelling that cannot live in
/// [`TYPE_SPELLINGS`] because a `const` cannot build the `Vec` a union needs.
/// [`Type::parse`] resolves it; this is here so the name has one home.
pub const NUMBER_TYPE_NAME: &str = "Number";

/// Builtin types that take parameters: `List<T>`, `Map<K, V>`, `Set<T>`, …
///
/// Names only. What each does with its parameters is `Type::parse`'s business,
/// and the arities differ; this is the list of *names* the editors have to know
/// about, which is the part that drifts.
pub const CONTAINER_TYPE_NAMES: &[&str] = &["List", "Map", "Set", "Tuple", "Task", "Channel", "Box", "Boxed"];

/// Answers "does this type implement this trait", for the assignability walk.
///
/// This crate has the `Type` and none of the declarations: a trait's name is
/// just a `Type::Named` here. The type checker holds the trait and impl tables
/// and implements this; [`NoTraits`] is the answer everywhere else, and is what
/// every existing caller of [`Type::is_assignable_to`] gets.
pub trait TraitOracle {
    fn implements(&self, ty: &Type, trait_name: &str) -> bool;
}

/// The oracle for a caller with no trait tables: nothing implements anything.
pub struct NoTraits;

impl TraitOracle for NoTraits {
    fn implements(&self, _ty: &Type, _trait_name: &str) -> bool {
        false
    }
}

impl Type {
    pub fn parse(s: &str) -> Option<Type> {
        Type::parse_at(s, 0)
    }

    /// [`Type::parse`], counting how deep it has gone.
    ///
    /// A type spelling nests without bound — `List<List<…<Int>…>>` — and this
    /// is a recursive descent over it, so a deep enough annotation overflowed
    /// the stack: `SIGABRT` and a core dump past about 1700 levels, on a
    /// program the tokenizer had accepted. The expression parser has had a
    /// bound for this reason; the type parser is the other half of the same
    /// surface, and the LSP and the browser playground read both from text they
    /// did not write.
    ///
    /// Past the bound is `None`, which every caller already words as "not a
    /// type" — the same answer a misspelling gets, and the reason this needs no
    /// new error path.
    fn parse_at(s: &str, depth: usize) -> Option<Type> {
        /// Deep enough that nothing written by hand comes close — the deepest
        /// annotation in this repository is four — and far enough under the
        /// measured overflow (between 1500 and 2000 levels) to stay there when
        /// a later walk over the type gets hungrier.
        const MAX_TYPE_DEPTH: usize = 128;
        if depth >= MAX_TYPE_DEPTH {
            return None;
        }
        let depth = depth + 1;
        let s = s.trim();

        // `_` — an element type that is not known. No positional rule keeps it
        // out of the top level: `let x: _ = 1;` parses and then fails to
        // type-check, because nothing is assignable to `_`. That is the same
        // answer a positional rule would give, from the type itself.
        if s == "_" {
            return Some(Type::Unknown);
        }

        // Handle primitive types
        if let Some((_, ty)) = PRIMITIVE_TYPES.iter().find(|(name, _)| *name == s) {
            return Some(ty.clone());
        }

        // A second spelling of one of them (`i64` is `Int`), checked before
        // `IntKind` so that `i64` does not become a *machine* int distinct from
        // the `Int` it is a spelling of.
        if let Some((_, ty)) = TYPE_SPELLINGS.iter().find(|(name, _)| *name == s) {
            return Some(ty.clone());
        }

        // `Number` is what the standard library's declarations have always
        // called `Int | Float`; until now it was a name the documentation could
        // write and the language could not.
        if s == NUMBER_TYPE_NAME {
            return Some(Type::Union(vec![Type::Int, Type::Float]));
        }

        if let Some(kind) = IntKind::parse(s) {
            return Some(Type::MachineInt(kind));
        }

        // The annotation parser joins tokens with spaces, so `*mut u32` can
        // arrive as `* mut u32`. Normalise the marker before matching, but keep
        // the space that separates `mut` from the pointee.
        let pointer_form = s.strip_prefix('*').map(|rest| alloc::format!("*{}", rest.trim_start()));
        let s = pointer_form.as_deref().unwrap_or(s);

        // `*mut T` before `*T`: the former's prefix is a superset.
        if let Some(rest) = s.strip_prefix("*mut ").or_else(|| s.strip_prefix("*mut")) {
            let pointee = Type::parse_at(rest.trim(), depth)?;
            return Some(Type::Ptr {
                pointee: Box::new(pointee),
                mutable: true,
            });
        }
        if let Some(rest) = s.strip_prefix('*') {
            let pointee = Type::parse_at(rest.trim(), depth)?;
            return Some(Type::Ptr {
                pointee: Box::new(pointee),
                mutable: false,
            });
        }

        // Handle type variables: 'T, 'K, 'V
        if s.starts_with('\'') && s.len() > 1 {
            return Some(Type::Variable(s[1..].to_string()));
        }

        // Handle function types before optional/union parsing so `(A) -> B?`
        // remains a function returning an optional rather than an optional
        // function type.
        if let Some(function_type) = parse_function_type(s) {
            return Some(function_type);
        }

        // Handle optional types: Int? (allow trailing whitespace before '?').
        let s_no_ws = s.trim_end();
        if let Some(inner) = s_no_ws.strip_suffix('?') {
            let inner = inner.trim_end();
            if !inner.is_empty() {
                return Type::parse_at(inner, depth).map(|t| Type::Optional(Box::new(t)));
            }
        }

        // Handle union types: Int | String | Nil
        if s.contains(" | ") {
            let parts = split_top_level(s, '|');
            if parts.len() == 1 {
                // The union separator is nested inside another type form such
                // as `List<Int | String>`; let that outer parser handle it.
            } else {
                let mut types = Vec::new();
                for part in parts {
                    if let Some(ty) = Type::parse_at(part, depth) {
                        types.push(ty);
                    }
                }
                if !types.is_empty() {
                    return Some(Type::Union(types));
                }
            }
        }

        // Handle generic types with angle brackets
        if let Some(open) = s.find('<')
            && let Some(close) = s.rfind('>')
        {
            let base = &s[..open];
            if !is_type_name(base) {
                return None;
            }
            let params_str = &s[open + 1..close];

            // Parse type parameters
            let params: Vec<Type> = if params_str.is_empty() {
                vec![]
            } else {
                let mut params = Vec::new();
                for param in split_top_level(params_str, ',') {
                    params.push(Type::parse_at(param, depth)?);
                }
                params
            };

            // Handle specific generic types
            match base {
                // `Tuple<..>` is a first-class variant, not a user generic. It was
                // missing here, so an annotation parsed as `Generic { name: "Tuple" }`
                // while a heterogeneous list literal infers `Type::Tuple` — two
                // different variants that `display()` renders identically, which is
                // why `fn f() -> Tuple<Bool, String> { return [true, "x"]; }` failed
                // with "expected Tuple<Bool, String>, got Tuple<Bool, String>".
                "Tuple" => return Some(Type::Tuple(params)),
                "List" => {
                    if params.len() == 1 {
                        return Some(Type::List(Box::new(params[0].clone())));
                    }
                }
                "Map" => {
                    if params.len() == 2 {
                        return Some(Type::Map(Box::new(params[0].clone()), Box::new(params[1].clone())));
                    }
                }
                "Set" => {
                    if params.len() == 1 {
                        return Some(Type::Set(Box::new(params[0].clone())));
                    }
                }
                "Task" => {
                    if params.len() == 1 {
                        return Some(Type::Task(Box::new(params[0].clone())));
                    }
                }
                "Channel" => {
                    if params.len() == 1 {
                        return Some(Type::Channel(Box::new(params[0].clone())));
                    }
                }
                "Box" | "Boxed" => {
                    if params.len() == 1 {
                        return Some(Type::Boxed(Box::new(params[0].clone())));
                    }
                }
                _ => {
                    // Generic custom type
                    return Some(Type::Generic {
                        name: base.to_string(),
                        params,
                    });
                }
            }
        }

        // Handle bare List and Map as generic types
        match s {
            "List" => Some(Type::List(Box::new(Type::Any))),
            "Map" => Some(Type::Map(Box::new(Type::Any), Box::new(Type::Any))),
            "Set" => Some(Type::Set(Box::new(Type::Any))),
            // A window is `Slice<Elem>`, and a bare `Slice` is the same
            // "whatever it holds" the three above mean. Without it `impl Slice`
            // typed `self` as a `Slice` with no element at all, which unified
            // with no receiver — so the block's methods could not be called,
            // and the diagnostic said the window had no such method.
            "Slice" => Some(Type::Generic {
                name: "Slice".to_string(),
                params: vec![Type::Any],
            }),
            _ => {
                // Assume it's a named custom type
                if is_type_name(s) {
                    Some(Type::Named(s.to_string()))
                } else {
                    None
                }
            }
        }
    }

    /// Get a display representation of the type
    pub fn display(&self) -> String {
        match self {
            Type::Int => "Int".to_string(),
            Type::Unknown => "_".to_string(),
            Type::MachineInt(kind) => kind.name().to_string(),
            Type::Ptr { pointee, mutable } => {
                if *mutable {
                    format!("*mut {}", pointee.display())
                } else {
                    format!("*{}", pointee.display())
                }
            }
            Type::Float => "Float".to_string(),
            Type::String => "String".to_string(),
            Type::Bool => "Bool".to_string(),
            Type::Nil => "Nil".to_string(),
            Type::Any => "Any".to_string(),
            Type::List(elem) => format!("List<{}>", elem.display()),
            Type::Map(k, v) => format!("Map<{}, {}>", k.display(), v.display()),
            Type::Set(elem) => format!("Set<{}>", elem.display()),
            Type::Tuple(elems) => {
                if elems.is_empty() {
                    "Tuple<>".to_string()
                } else {
                    let mut parts = Vec::with_capacity(elems.len());
                    for elem in elems {
                        parts.push(elem.display());
                    }
                    format!("Tuple<{}>", parts.join(", "))
                }
            }
            Type::Function {
                params,
                named_params,
                return_type,
            } => {
                let mut segments: Vec<String> = Vec::new();
                if !params.is_empty() {
                    for param in params {
                        segments.push(param.display());
                    }
                }
                if !named_params.is_empty() {
                    let mut named_parts = Vec::with_capacity(named_params.len());
                    for np in named_params {
                        let mut s = format!("{}: {}", np.name, np.ty.display());
                        if np.has_default {
                            s.push_str(" = _");
                        }
                        named_parts.push(s);
                    }
                    segments.push(format!("{{{}}}", named_parts.join(", ")));
                }
                format!("({}) -> {}", segments.join(", "), return_type.display())
            }
            Type::Task(inner) => format!("Task<{}>", inner.display()),
            Type::Channel(inner) => format!("Channel<{}>", inner.display()),
            Type::Union(types) => {
                let mut type_strs = Vec::with_capacity(types.len());
                for ty in types {
                    type_strs.push(ty.display());
                }
                type_strs.join(" | ")
            }
            Type::Optional(inner) => format!("{}?", inner.display()),
            Type::Variable(name) => format!("'{}", name),
            Type::Named(name) => name.clone(),
            Type::Generic { name, params } => {
                if params.is_empty() {
                    name.clone()
                } else {
                    let mut param_strs = Vec::with_capacity(params.len());
                    for param in params {
                        param_strs.push(param.display());
                    }
                    format!("{}<{}>", name, param_strs.join(", "))
                }
            }
            Type::Boxed(inner) => format!("Box<{}>", inner.display()),
        }
    }

    /// Whether a container whose element type is `source` may be used where
    /// one whose element type is `target` is expected.
    ///
    /// Invariant, with two exceptions that are not variance:
    ///
    /// - `target` is `_` — the container is being read, never written, so any
    ///   element type is fine. This is the whole reason `_` exists.
    /// - either side is still a free type variable — `let xs: List<Int> = [];`
    ///   gives the empty literal `List<'T>`, and binding `'T` to `Int` is
    ///   inference, not a widening of one container into another.
    fn element_assignable_with(source: &Type, target: &Type, oracle: &dyn TraitOracle) -> bool {
        match (source, target) {
            (_, Type::Unknown) => true,
            (Type::Variable(_), _) | (_, Type::Variable(_)) => source.is_assignable_to_with(target, oracle),
            _ => source == target,
        }
    }

    /// Whether a container of `self` may fill a `target` **because it is a
    /// literal** — a container the program has no other name for.
    ///
    /// Containers are invariant because a widening is an alias: two names for
    /// one object, disagreeing about the element type, and the wider one can
    /// write what the narrower one's type forbids. A literal has no second
    /// name, so its elements are checked covariantly, exactly as they were
    /// before, and `let xs: List<Any> = [1, 2];` or `f([1, 2])` still work.
    ///
    /// The counterpart of the machine-int literal rule (`let x: u8 = 5` rather
    /// than `5 as u8`), for the same reason and at the same three positions:
    /// a `let` with an annotation, a positional argument, and a named one.
    /// Callers pass the *expression* so that only a literal takes this path.
    pub fn container_literal_fits(&self, target: &Type) -> bool {
        self.container_literal_fits_with(target, &NoTraits)
    }

    /// [`Self::container_literal_fits`] with a [`TraitOracle`], for the same
    /// reason [`Self::is_assignable_to_with`] takes one: the elements may be
    /// required to implement a trait.
    pub fn container_literal_fits_with(&self, target: &Type, oracle: &dyn TraitOracle) -> bool {
        match (self, target) {
            (Type::List(a), Type::List(b)) => a.is_assignable_to_with(b, oracle),
            (Type::Set(a), Type::Set(b)) => a.is_assignable_to_with(b, oracle),
            (Type::Map(ak, av), Type::Map(bk, bv)) => {
                ak.is_assignable_to_with(bk, oracle) && av.is_assignable_to_with(bv, oracle)
            }
            // A heterogeneous literal infers as a `Tuple` — that is the whole
            // reason the variant exists — so `[p, q]` written for a
            // `List<Show>` never reached the `List` arm above and the
            // annotation was rejected by the precision it asked for. Element by
            // element, like the arms above, and covariant for the same reason:
            // a literal has no second name.
            (Type::Tuple(elems), Type::List(target)) => {
                elems.iter().all(|elem| elem.is_assignable_to_with(target, oracle))
            }
            _ => false,
        }
    }

    /// Check if this type can be assigned to another type (subtyping)
    pub fn is_assignable_to(&self, other: &Type) -> bool {
        self.is_assignable_to_with(other, &NoTraits)
    }

    /// [`Self::is_assignable_to`] with a [`TraitOracle`] for the one question
    /// this crate cannot answer on its own: whether a type implements a named
    /// trait. The rule belongs in this walk — a trait may be the target
    /// anywhere a type may — and the tables that answer it live in the type
    /// checker, so it arrives as a parameter rather than as a second, partial
    /// copy of the walk over there.
    pub fn is_assignable_to_with(&self, other: &Type, oracle: &dyn TraitOracle) -> bool {
        match (self, other) {
            // Any type is assignable to Any
            (_, Type::Any) => true,
            // Any can flow into any type (dynamic fallback)
            (Type::Any, _) => true,
            // A value whose type is still a free variable can become what is
            // expected of it. `let xs: List<Int> = [];` is the case that
            // matters: an empty literal has no element to infer from, so its
            // type is `List<'T>`, and recursing into the element compared `'T`
            // against `Int` and fell through to "no rule" — an annotation being
            // *rejected* by the very absence of information it was written to
            // supply.
            //
            // Source side only. A variable here means the value has not been
            // decided yet, which is a thing an annotation may decide; a
            // variable on the *target* side would mean the annotation itself is
            // undetermined, and accepting anything into it would make a generic
            // parameter a hole rather than a constraint. This is not where
            // unification happens either way — the constraint solver runs after
            // and is what rejects a variable that two uses pull apart.
            (Type::Variable(_), _) => true,
            // Same types are assignable
            (a, b) if a == b => true,
            // Boxed types act as transparent wrappers — must come before numeric hierarchy
            // so that Box<Any> unwraps to Any before numeric ordering is applied.
            (Type::Boxed(inner), Type::Boxed(expected)) => inner.is_assignable_to_with(expected, oracle),
            (Type::Boxed(inner), expected) => inner.is_assignable_to_with(expected, oracle),
            (actual, Type::Boxed(expected)) => actual.is_assignable_to_with(expected, oracle),
            // Machine integers convert only explicitly, in either direction and
            // even between two machine widths. Systems code is exactly where an
            // implicit narrowing or sign change is a bug rather than a
            // convenience, and `u8 -> Int` silently promoting would defeat the
            // point of asking for a fixed width. `as` is the way across.
            (Type::MachineInt(_), _) | (_, Type::MachineInt(_)) => false,
            // Nullability is not a numeric property. The hierarchy rule below
            // asks `numeric_class`, which looks *through* `Optional` (it must —
            // it answers "what does arithmetic on this produce"), so `Int?` and
            // `Int` both classified as Int and `let n: Int = xs.index_of(x);`
            // was accepted. The nil then travelled to whatever used `n` and
            // failed there instead, which is exactly what `?` exists to
            // prevent — and `String?` was already rejected in the same
            // position, so the rule only had a hole for numbers.
            (lhs, rhs) if lhs.may_be_nil() && !rhs.may_be_nil() => false,
            // Numeric hierarchy: allow Int -> Float, Float -> Boxed, etc.
            (lhs, rhs) if lhs.numeric_class().is_some() && rhs.numeric_class().is_some() => {
                let lhs_class = lhs.numeric_class().unwrap();
                let rhs_class = rhs.numeric_class().unwrap();
                lhs_class <= rhs_class
            }
            (Type::Nil, Type::Optional(_)) => true,
            // Optional types: T is assignable to ?T
            (inner, Type::Optional(expected_inner)) => inner.is_assignable_to_with(expected_inner, oracle),
            // Union types: T is assignable to Union if T is assignable to any member
            (t, Type::Union(union_types)) => union_types.iter().any(|ut| t.is_assignable_to_with(ut, oracle)),
            // Union member is assignable to union
            (Type::Union(union_types), target) => union_types.iter().all(|ut| ut.is_assignable_to_with(target, oracle)),
            // Containers are **invariant** in their element types, and the
            // read-only view `List<_>` is how a signature says "a list of
            // anything" without them.
            //
            // Covariance here was unsound, because these containers are mutable
            // and a widening is an *alias*: `let b: List<Any> = a;` then
            // `b.push("s")` put a String into an `List<Int>`, and
            // `let c: Int = a[2]` type-checked and held it. Five widening
            // positions did it — a `let`, a parameter, a struct field, a
            // container element, and a return type — so restricting any one of
            // them would not have been enough.
            (Type::List(a), Type::List(b)) => Self::element_assignable_with(a, b, oracle),
            (Type::Map(ak, av), Type::Map(bk, bv)) => {
                Self::element_assignable_with(ak, bk, oracle) && Self::element_assignable_with(av, bv, oracle)
            }
            (Type::Set(a), Type::Set(b)) => Self::element_assignable_with(a, b, oracle),
            // The same rule for a parameterised named type — `Slice<T>` is the
            // only one today. Without an element rule at all, `Slice<Int>` was
            // assignable to nothing but itself, so a declaration could not
            // accept "a window over anything".
            (
                Type::Generic {
                    name: a_name,
                    params: a_params,
                },
                Type::Generic {
                    name: b_name,
                    params: b_params,
                },
            ) => {
                a_name == b_name
                    && a_params.len() == b_params.len()
                    && a_params
                        .iter()
                        .zip(b_params)
                        .all(|(a, b)| Self::element_assignable_with(a, b, oracle))
            }
            (Type::Tuple(as_), Type::Tuple(bs)) => {
                as_.len() == bs.len()
                    && as_
                        .iter()
                        .zip(bs.iter())
                        .all(|(a, b)| a.is_assignable_to_with(b, oracle))
            }
            // A tuple *is* a list. `Tuple` is not a runtime thing — `HeapValue`
            // has `List` and no tuple at all; the variant exists so a
            // heterogeneous literal can keep each element's type instead of
            // collapsing to `List<Any>`. Without this rule that extra precision
            // reads as a different type, and `let xs: List = [1, "a"];` — an
            // ordinary list in a language whose lists are heterogeneous — was
            // rejected by the annotation written to describe it.
            (Type::Tuple(elems), Type::List(target)) => elems
                .iter()
                .all(|elem| Self::element_assignable_with(elem, target, oracle)),
            // And the way back, which was missing — so `Tuple<Int, Int>` was a
            // type nothing could satisfy: `[1, 2]` is `List<Int>` (its elements
            // do not differ, so no tuple is inferred), and without this rule it
            // was not assignable to the annotation written to describe it.
            // `Tuple<Int, String>` looked fine only because a *heterogeneous*
            // literal infers `Tuple` directly and never needed the conversion.
            //
            // The unifier has had both directions all along, in one arm with
            // both orders — so this was also the two of them disagreeing, which
            // is the thing the note over there says must not happen.
            //
            // Length is deliberately not part of it: a `List<T>` type carries no
            // length, so there is nothing to compare against the tuple's arity.
            // The precision a tuple adds is *per-position element types*, and
            // that is what this checks.
            (Type::List(source), Type::Tuple(elems)) => elems
                .iter()
                .all(|elem| Self::element_assignable_with(source, elem, oracle)),
            // Function types (contravariant parameters, covariant return)
            (
                Type::Function {
                    params: a_params,
                    named_params: a_named,
                    return_type: a_ret,
                },
                Type::Function {
                    params: b_params,
                    named_params: b_named,
                    return_type: b_ret,
                },
            ) => {
                if a_params.len() != b_params.len() {
                    false
                } else {
                    // Parameters are contravariant
                    let params_compatible = b_params
                        .iter()
                        .zip(a_params.iter())
                        .all(|(b_param, a_param)| b_param.is_assignable_to_with(a_param, oracle));
                    if !params_compatible {
                        return false;
                    }

                    if a_named.len() != b_named.len() {
                        return false;
                    }
                    let mut a_map = HashMap::<&str, &FunctionNamedParamType>::with_capacity(a_named.len());
                    for np in a_named {
                        a_map.insert(np.name.as_str(), np);
                    }
                    let named_compatible = b_named.iter().all(|b_np| {
                        if let Some(a_np) = a_map.get(b_np.name.as_str()) {
                            b_np.has_default == a_np.has_default && b_np.ty.is_assignable_to_with(&a_np.ty, oracle)
                        } else {
                            false
                        }
                    });
                    if !named_compatible {
                        return false;
                    }
                    // Return type is covariant
                    let return_compatible = a_ret.is_assignable_to_with(b_ret, oracle);
                    params_compatible && named_compatible && return_compatible
                }
            }
            // Concurrency types
            (Type::Task(a), Type::Task(b)) => a.is_assignable_to_with(b, oracle),
            (Type::Channel(a), Type::Channel(b)) => a.is_assignable_to_with(b, oracle),
            // A trait names a type, and whatever implements it may stand where
            // it is expected. Last, so it costs nothing until every structural
            // rule has already declined — and only the *oracle* decides, so a
            // build with no trait tables behaves exactly as before.
            (from, Type::Named(trait_name)) => oracle.implements(from, trait_name),
            // No other assignability rules
            _ => false,
        }
    }

    /// Map type into numeric hierarchy class when applicable.
    pub fn numeric_class(&self) -> Option<NumericClass> {
        NumericHierarchy::classify(self)
    }

    /// Whether a value of this type can be `nil`.
    ///
    /// `Any` says no: it is *unknown*, not nullable, and assignability already
    /// lets it flow both ways before this is consulted.
    pub fn may_be_nil(&self) -> bool {
        match self {
            Type::Nil | Type::Optional(_) => true,
            Type::Union(items) => items.iter().any(Type::may_be_nil),
            Type::Boxed(inner) => inner.may_be_nil(),
            _ => false,
        }
    }

    /// Check if this type contains any type variables
    pub fn contains_variables(&self) -> bool {
        match self {
            Type::Variable(_) => true,
            Type::List(inner) | Type::Set(inner) | Type::Optional(inner) | Type::Task(inner) | Type::Channel(inner) => {
                inner.contains_variables()
            }
            Type::Map(k, v) => k.contains_variables() || v.contains_variables(),
            Type::Function {
                params,
                named_params,
                return_type,
            } => {
                params.iter().any(|p| p.contains_variables())
                    || named_params.iter().any(|np| np.ty.contains_variables())
                    || return_type.contains_variables()
            }
            Type::Union(types) => types.iter().any(|t| t.contains_variables()),
            Type::Tuple(elems) => elems.iter().any(|t| t.contains_variables()),
            Type::Generic { params, .. } => params.iter().any(|p| p.contains_variables()),
            Type::Boxed(inner) => inner.contains_variables(),
            _ => false,
        }
    }

    /// Every type variable name occurring in this type, in order, without
    /// duplicates.
    ///
    /// [`contains_variables`] answers whether there are any; this answers
    /// *which*, which is what instantiating a generic signature needs — each
    /// one gets a fresh copy, consistently across the whole signature so that
    /// `fn first(xs) { return xs[0]; }`'s `List<'a> -> 'a` stays one relation
    /// rather than two unrelated holes.
    ///
    /// [`contains_variables`]: Type::contains_variables
    pub fn collect_variables(&self, out: &mut Vec<String>) {
        match self {
            Type::Variable(name) => {
                if !out.iter().any(|seen| seen == name) {
                    out.push(name.clone());
                }
            }
            Type::List(inner)
            | Type::Set(inner)
            | Type::Optional(inner)
            | Type::Task(inner)
            | Type::Channel(inner)
            | Type::Boxed(inner) => inner.collect_variables(out),
            Type::Ptr { pointee, .. } => pointee.collect_variables(out),
            Type::Map(k, v) => {
                k.collect_variables(out);
                v.collect_variables(out);
            }
            Type::Function {
                params,
                named_params,
                return_type,
            } => {
                for param in params {
                    param.collect_variables(out);
                }
                for named in named_params {
                    named.ty.collect_variables(out);
                }
                return_type.collect_variables(out);
            }
            Type::Union(types) | Type::Tuple(types) => {
                for ty in types {
                    ty.collect_variables(out);
                }
            }
            Type::Generic { params, .. } => {
                for param in params {
                    param.collect_variables(out);
                }
            }
            _ => {}
        }
    }

    /// Substitute type variables with concrete types
    pub fn substitute(&self, substitutions: &HashMap<String, Type>) -> Type {
        match self {
            Type::Variable(name) => substitutions.get(name).cloned().unwrap_or_else(|| self.clone()),
            Type::List(inner) => Type::List(Box::new(inner.substitute(substitutions))),
            Type::Set(inner) => Type::Set(Box::new(inner.substitute(substitutions))),
            Type::Map(k, v) => Type::Map(
                Box::new(k.substitute(substitutions)),
                Box::new(v.substitute(substitutions)),
            ),
            Type::Function {
                params,
                named_params,
                return_type,
            } => Type::Function {
                params: {
                    let mut out = Vec::with_capacity(params.len());
                    for param in params {
                        out.push(param.substitute(substitutions));
                    }
                    out
                },
                named_params: {
                    let mut out = Vec::with_capacity(named_params.len());
                    for np in named_params {
                        out.push(FunctionNamedParamType {
                            name: np.name.clone(),
                            ty: np.ty.substitute(substitutions),
                            has_default: np.has_default,
                        });
                    }
                    out
                },
                return_type: Box::new(return_type.substitute(substitutions)),
            },
            Type::Tuple(elems) => {
                let mut out = Vec::with_capacity(elems.len());
                for elem in elems {
                    out.push(elem.substitute(substitutions));
                }
                Type::Tuple(out)
            }
            Type::Optional(inner) => Type::Optional(Box::new(inner.substitute(substitutions))),
            Type::Task(inner) => Type::Task(Box::new(inner.substitute(substitutions))),
            Type::Channel(inner) => Type::Channel(Box::new(inner.substitute(substitutions))),
            Type::Union(types) => {
                let mut out = Vec::with_capacity(types.len());
                for ty in types {
                    out.push(ty.substitute(substitutions));
                }
                Type::Union(out)
            }
            Type::Generic { name, params } => Type::Generic {
                name: name.clone(),
                params: {
                    let mut out = Vec::with_capacity(params.len());
                    for param in params {
                        out.push(param.substitute(substitutions));
                    }
                    out
                },
            },
            Type::Boxed(inner) => Type::Boxed(Box::new(inner.substitute(substitutions))),
            _ => self.clone(),
        }
    }
}

fn is_type_name(value: &str) -> bool {
    let mut chars = value.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn parse_function_type(s: &str) -> Option<Type> {
    let arrow = find_top_level_arrow(s)?;
    let params_str = s[..arrow].trim();
    let return_str = s[arrow + 2..].trim();
    if !params_str.starts_with('(') || !params_str.ends_with(')') || return_str.is_empty() {
        return None;
    }

    let inner = &params_str[1..params_str.len() - 1];
    let (params, named_params) = parse_function_param_types(inner)?;
    let return_type = Type::parse(return_str)?;
    Some(Type::Function {
        params,
        named_params,
        return_type: Box::new(return_type),
    })
}

fn parse_function_param_types(s: &str) -> Option<(Vec<Type>, Vec<FunctionNamedParamType>)> {
    let mut params = Vec::new();
    let mut named_params = Vec::new();
    let s = s.trim();
    if s.is_empty() {
        return Some((params, named_params));
    }

    for part in split_top_level(s, ',') {
        let part = part.trim();
        if part.is_empty() {
            return None;
        }
        if part.starts_with('{') || part.ends_with('}') {
            if !part.starts_with('{') || !part.ends_with('}') {
                return None;
            }
            let block = &part[1..part.len() - 1];
            for named in split_top_level(block, ',') {
                named_params.push(parse_function_named_param_type(named)?);
            }
        } else {
            params.push(Type::parse(part)?);
        }
    }

    Some((params, named_params))
}

fn parse_function_named_param_type(s: &str) -> Option<FunctionNamedParamType> {
    let colon = find_top_level_char(s, ':')?;
    let name = s[..colon].trim();
    if !is_type_name(name) {
        return None;
    }
    let rest = s[colon + 1..].trim();
    let (ty, has_default) = if let Some(assign) = find_top_level_char(rest, '=') {
        (rest[..assign].trim(), true)
    } else {
        (rest, false)
    };
    if ty.is_empty() {
        return None;
    }
    Some(FunctionNamedParamType {
        name: name.to_string(),
        ty: Type::parse(ty)?,
        has_default,
    })
}

fn find_top_level_arrow(s: &str) -> Option<usize> {
    let mut paren = 0i32;
    let mut bracket = 0i32;
    let mut brace = 0i32;
    let mut angle = 0i32;
    let mut chars = s.char_indices().peekable();
    while let Some((index, ch)) = chars.next() {
        match ch {
            '(' => paren += 1,
            ')' => paren -= 1,
            '[' => bracket += 1,
            ']' => bracket -= 1,
            '{' => brace += 1,
            '}' => brace -= 1,
            '<' => angle += 1,
            '>' => angle -= 1,
            '-' if paren == 0
                && bracket == 0
                && brace == 0
                && angle == 0
                && chars.peek().is_some_and(|(_, next)| *next == '>') =>
            {
                return Some(index);
            }
            _ => {}
        }
    }
    None
}

fn find_top_level_char(s: &str, needle: char) -> Option<usize> {
    let mut paren = 0i32;
    let mut bracket = 0i32;
    let mut brace = 0i32;
    let mut angle = 0i32;
    for (index, ch) in s.char_indices() {
        match ch {
            '(' => paren += 1,
            ')' => paren -= 1,
            '[' => bracket += 1,
            ']' => bracket -= 1,
            '{' => brace += 1,
            '}' => brace -= 1,
            '<' => angle += 1,
            '>' => angle -= 1,
            _ if ch == needle && paren == 0 && bracket == 0 && brace == 0 && angle == 0 => {
                return Some(index);
            }
            _ => {}
        }
    }
    None
}

fn split_top_level(s: &str, delimiter: char) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0usize;
    let mut paren = 0i32;
    let mut bracket = 0i32;
    let mut brace = 0i32;
    let mut angle = 0i32;
    for (index, ch) in s.char_indices() {
        match ch {
            '(' => paren += 1,
            ')' => paren -= 1,
            '[' => bracket += 1,
            ']' => bracket -= 1,
            '{' => brace += 1,
            '}' => brace -= 1,
            '<' => angle += 1,
            '>' => angle -= 1,
            _ if ch == delimiter && paren == 0 && bracket == 0 && brace == 0 && angle == 0 => {
                parts.push(s[start..index].trim());
                start = index + ch.len_utf8();
            }
            _ => {}
        }
    }
    parts.push(s[start..].trim());
    parts
}

#[cfg(test)]
mod tests {
    /// A type spelling too deep to walk is refused, not a core dump.
    ///
    /// `Type::parse` is a recursive descent over the spelling, so
    /// `List<List<…<Int>…>>` overflowed the stack past about 1700 levels —
    /// `SIGABRT`, on a program the tokenizer had accepted. The expression
    /// parser has had a bound for this reason and this is the other half of the
    /// same surface: the LSP and the browser playground read both from text
    /// they did not write.
    #[test]
    fn a_type_too_deep_is_refused_not_aborted() {
        // Four is the deepest annotation this repository writes; a hundred is
        // past anything and still parses.
        let ok = format!("{}Int{}", "List<".repeat(100), ">".repeat(100));
        assert!(Type::parse(&ok).is_some(), "a hundred levels still parses");

        // Past the bound is `None` — "not a type", the answer a misspelling
        // gets — at any size.
        for depth in [200, 3000, 20_000] {
            let deep = format!("{}Int{}", "List<".repeat(depth), ">".repeat(depth));
            assert!(
                Type::parse(&deep).is_none(),
                "{depth} levels must be refused, not walked"
            );
        }
    }

    use super::{IntKind, ShortStr, ShortStrOrStr, Type};
    use alloc::boxed::Box;
    use alloc::format;
    use alloc::string::ToString;
    use alloc::vec;

    #[test]
    fn short_str_concat_int_falls_back_when_prefix_fills_inline_buffer() {
        let prefix = ShortStr::new("answer=").expect("short");

        let value = prefix.concat_int(42);

        match value {
            ShortStrOrStr::Str(value) => assert_eq!(value, "answer=42"),
            ShortStrOrStr::Short(value) => panic!("expected heap string fallback, got {}", value.as_str()),
        }
    }

    #[test]
    fn machine_int_kinds_round_trip_through_their_spelling() {
        for kind in [
            IntKind::I8,
            IntKind::I16,
            IntKind::I32,
            IntKind::I64,
            IntKind::U8,
            IntKind::U16,
            IntKind::U32,
            IntKind::U64,
            IntKind::Isize,
            IntKind::Usize,
        ] {
            assert_eq!(IntKind::parse(kind.name()), Some(kind), "{}", kind.name());
            // `i64` is the exception, and deliberately: it is a second spelling
            // of `Int` rather than a machine int of its own, so that a driver's
            // `i64` and a front end's `Int` are one type instead of two that
            // need a cast between them (see `TYPE_SPELLINGS`).
            let expected = if kind == IntKind::I64 {
                Type::Int
            } else {
                Type::MachineInt(kind)
            };
            assert_eq!(Type::parse(kind.name()), Some(expected), "{}", kind.name());
            assert_eq!(Type::MachineInt(kind).display(), kind.name());
        }
    }

    #[test]
    fn machine_int_ranges_match_their_width() {
        assert_eq!(IntKind::U8.range(), Some((0, 255)));
        assert_eq!(IntKind::I8.range(), Some((-128, 127)));
        assert_eq!(IntKind::U32.range(), Some((0, 4_294_967_295)));
        assert_eq!(IntKind::I32.range(), Some((-2_147_483_648, 2_147_483_647)));
        // Pointer width is a property of the target, not of the type.
        assert_eq!(IntKind::Usize.bits(), None);
        assert_eq!(IntKind::Usize.range(), None);
    }

    /// Machine integers convert only explicitly. An implicit narrowing or sign
    /// change is a bug in exactly the code that asks for a fixed width, and an
    /// implicit widening to `Int` would defeat the point of asking.
    #[test]
    fn machine_ints_never_convert_implicitly() {
        let u8_ = Type::MachineInt(IntKind::U8);
        let u32_ = Type::MachineInt(IntKind::U32);
        let i32_ = Type::MachineInt(IntKind::I32);

        // Not even the lossless widening.
        assert!(!u8_.is_assignable_to(&u32_));
        assert!(!u8_.is_assignable_to(&Type::Int));
        assert!(!Type::Int.is_assignable_to(&u8_));
        // Nor across signedness at equal width.
        assert!(!i32_.is_assignable_to(&u32_));
        assert!(!u32_.is_assignable_to(&i32_));
        // Nor into the float hierarchy.
        assert!(!i32_.is_assignable_to(&Type::Float));

        // Identity still holds.
        assert!(u8_.is_assignable_to(&u8_));
    }

    #[test]
    fn pointer_types_round_trip_through_their_spelling() {
        let u8_ptr = Type::Ptr {
            pointee: Box::new(Type::MachineInt(IntKind::U8)),
            mutable: false,
        };
        let u32_mut = Type::Ptr {
            pointee: Box::new(Type::MachineInt(IntKind::U32)),
            mutable: true,
        };
        assert_eq!(Type::parse("*u8"), Some(u8_ptr.clone()));
        assert_eq!(Type::parse("*mut u32"), Some(u32_mut.clone()));
        assert_eq!(u8_ptr.display(), "*u8");
        assert_eq!(u32_mut.display(), "*mut u32");
        // The annotation parser joins tokens with spaces.
        assert_eq!(Type::parse("* mut u32"), Some(u32_mut));
    }

    #[test]
    fn pointers_nest() {
        let nested = Type::parse("*mut *u8").expect("parses");
        let Type::Ptr { pointee, mutable } = &nested else {
            panic!("expected a pointer, got {nested:?}");
        };
        assert!(mutable);
        assert!(matches!(pointee.as_ref(), Type::Ptr { mutable: false, .. }));
        assert_eq!(nested.display(), "*mut *u8");
    }

    /// Mutability is part of the type: a read-only pointer must not satisfy a
    /// `*mut` annotation, or a register marked read-only could be written.
    #[test]
    fn pointer_mutability_is_not_assignable_away() {
        let read = Type::parse("*u32").expect("parses");
        let write = Type::parse("*mut u32").expect("parses");
        assert!(!read.is_assignable_to(&write));
        assert!(!write.is_assignable_to(&read));
        assert!(read.is_assignable_to(&read));
    }

    /// `Any` is the dynamic escape hatch and stays above the rule, otherwise a
    /// machine int could never flow through untyped code at all.
    #[test]
    fn machine_ints_still_interoperate_with_any() {
        let u8_ = Type::MachineInt(IntKind::U8);
        assert!(u8_.is_assignable_to(&Type::Any));
        assert!(Type::Any.is_assignable_to(&u8_));
    }

    /// One type, two spellings — so a driver's `i64` and a front end's `Int`
    /// are the same value and pass through each other's functions.
    ///
    /// Two *convertible* types would be more ambiguity, not less: a reader
    /// would have to know which one a value is to know what it does.
    #[test]
    fn a_widthed_spelling_and_a_plain_one_name_the_same_type() {
        assert_eq!(Type::parse("i64"), Some(Type::Int));
        assert_eq!(Type::parse("f64"), Some(Type::Float));
        assert!(Type::Int.is_assignable_to(&Type::parse("i64").unwrap()));
        assert!(Type::parse("i64").unwrap().is_assignable_to(&Type::Int));
    }

    /// `isize` is deliberately *not* one of them.
    ///
    /// Pointer width is the entire reason that name exists, and equating it
    /// with a fixed width would put the language's plain integer at the mercy
    /// of the target: on `thumbv7em-none-eabi` it is 32 bits while the VM's
    /// `RuntimeVal::Int` is still an `i64`. One name, two widths.
    #[test]
    fn pointer_width_is_its_own_type() {
        assert_eq!(Type::parse("isize"), Some(Type::MachineInt(IntKind::Isize)));
        assert_eq!(Type::parse("usize"), Some(Type::MachineInt(IntKind::Usize)));
        assert_ne!(Type::parse("isize"), Some(Type::Int));
    }

    #[test]
    fn number_is_int_or_float() {
        assert_eq!(Type::parse("Number"), Some(Type::Union(vec![Type::Int, Type::Float])));
        assert!(Type::Int.is_assignable_to(&Type::parse("Number").unwrap()));
        assert!(Type::Float.is_assignable_to(&Type::parse("Number").unwrap()));
        assert!(!Type::String.is_assignable_to(&Type::parse("Number").unwrap()));
    }

    /// Every way a `ShortStr` can come into existence produces valid UTF-8.
    ///
    /// `as_str` skips the check and reads the bytes directly, so this is the
    /// thing that has to stay true. The `debug_assert!` inside `as_str` does
    /// the actual verifying — this test's job is to *reach* it from each
    /// constructor, including the multi-byte cases a byte-length limit is most
    /// likely to cut in half.
    #[test]
    fn every_short_str_constructor_keeps_the_utf8_invariant() {
        for text in ["", "a", "abc", "1234567", "中", "中中", "é", "aé", "\u{7f}", "\u{80}"] {
            match ShortStr::new(text) {
                Some(short) => assert_eq!(short.as_str(), text),
                // Over seven bytes: refused, which is the other half of the
                // invariant (a truncating constructor could split a character).
                None => assert!(text.len() > 7, "{text:?} fits but was refused"),
            }
        }
        for ch in ['a', '中', 'é', '\u{10FFFF}', '\u{0}'] {
            assert_eq!(ShortStr::from_char(ch).as_str().chars().next(), Some(ch));
        }
        let base = ShortStr::new("ab").expect("fits");
        for n in [0i64, 7, 9999, 10_000, -1, i64::MIN] {
            let joined = match base.concat_int(n) {
                ShortStrOrStr::Short(short) => short.as_str().to_string(),
                ShortStrOrStr::Str(text) => text,
            };
            assert_eq!(joined, format!("ab{n}"));
            let prefixed = match ShortStr::concat_int_prefix(n, base) {
                ShortStrOrStr::Short(short) => short.as_str().to_string(),
                ShortStrOrStr::Str(text) => text,
            };
            assert_eq!(prefixed, format!("{n}ab"));
        }
        // Concatenation across the seven-byte edge, with a multi-byte operand
        // on each side.
        let multi = ShortStr::new("中").expect("three bytes fit");
        for (left, right) in [(base, multi), (multi, base), (multi, multi)] {
            let joined = match left.concat(right) {
                ShortStrOrStr::Short(short) => short.as_str().to_string(),
                ShortStrOrStr::Str(text) => text,
            };
            assert_eq!(joined, format!("{}{}", left.as_str(), right.as_str()));
        }
    }
}
