//! A closure as a **runtime value**.
//!
//! Every other closure in the native build is a compile-time fact: the lowering
//! knows which function a register names, so a call devirtualizes and the
//! captures become hidden trailing arguments. That covers a closure that is
//! built and called, which is most of them — and nothing else. Storing one in a
//! list, putting one in a struct field, or returning one from a branch has no
//! compile-time answer.
//!
//! This is the value they become. It is the shape `spawn` already used to reach
//! a lambda through a pointer: the callee is a lowered `lk_fn_N` whose
//! signature the lowering pinned to all-`LkDyn`, so one arity switch can call
//! any of them. The environment travels beside the pointer instead of as hidden
//! arguments, and the call appends it — which is exactly the argument order the
//! native signature already has (`params…`, then `captures…`).
//!
//! Owned, not borrowed: a closure outlives the frame that built it by
//! definition, so its captures are deep-copied into `OwnedVal` the way a
//! spawned goroutine's are, and re-materialized into the caller's arena on each
//! call.

use alloc::boxed::Box;
use alloc::vec::Vec;
use core::ffi::c_void;

use crate::chan::{OwnedVal, materialize};
use crate::lkdyn::{DYN_CLOSURE, LkDyn};

/// A callable value: where the code is, how many arguments it takes, and what
/// it captured.
pub(crate) struct LkClosure {
    /// A lowered `lk_fn_N`, whose signature is `(LkDyn × (params + env)) -> LkDyn`.
    pub(crate) code: *const c_void,
    /// Visible parameters. The environment's length is `env.len()`, and the two
    /// together are the native arity.
    pub(crate) params: i64,
    /// The module function index, carried only so `display` can print what the
    /// interpreter prints: `<fn #3(1 captures)>`.
    pub(crate) fn_index: i64,
    pub(crate) env: Vec<OwnedVal>,
}

/// Builds one from a function address and an argument block of captures.
///
/// The block is the same `lkrt_spawn_args_new`/`push` pair a `spawn` builds,
/// and ownership of it moves here.
///
/// # Safety
/// `env_block` must be a live handle from `lkrt_spawn_args_new`, or null for a
/// capture-free lambda.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_closure_new(
    code: *const c_void,
    env_block: *mut c_void,
    params: i64,
    fn_index: i64,
) -> LkDyn {
    let env = if env_block.is_null() {
        Vec::new()
    } else {
        // SAFETY: ownership of the block moves here, as it does into a spawn.
        *unsafe { Box::from_raw(env_block as *mut Vec<OwnedVal>) }
    };
    LkDyn {
        tag: DYN_CLOSURE,
        payload: crate::state::arena_handle(LkClosure {
            code,
            params,
            fn_index,
            env,
        }) as i64,
    }
}

/// How many arguments the closure takes.
///
/// # Safety
/// `callee` must be a `DYN_CLOSURE` value.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_closure_arity(callee: LkDyn) -> i64 {
    // SAFETY: the tag is only set by `lkrt_closure_new`.
    unsafe { closure_of(callee) }.params
}

/// Calls it with an argument block, appending the captured environment.
///
/// The arity switch mirrors `spawn`'s, and for the same reason: a `LkDyn` is
/// two machine words, so there is no variadic form to call through and each
/// arity needs its own signature.
///
/// # Safety
/// `callee` must be a `DYN_CLOSURE` value and `args_block` a live handle from
/// `lkrt_spawn_args_new` (ownership moves here), or null for no arguments.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_closure_call(callee: LkDyn, args_block: *mut c_void) -> LkDyn {
    // Checked before the block is taken apart, so calling a non-callable says
    // so rather than first consuming arguments it will never pass.
    // SAFETY: as documented.
    unsafe { closure_of(callee) };
    let mut args: Vec<LkDyn> = if args_block.is_null() {
        Vec::new()
    } else {
        // SAFETY: ownership of the block moves here.
        let block = *unsafe { Box::from_raw(args_block as *mut Vec<OwnedVal>) };
        block.iter().map(materialize).collect()
    };
    // SAFETY: as documented.
    unsafe { call_with(callee, &mut args) }
}

/// The call itself, once the arguments are in a `Vec`.
///
/// Split out so a caller that already has the arguments — the list HOFs with a
/// closure callback — does not have to allocate an argument *block* just to
/// have this function take it apart again.
///
/// # Safety
/// `callee` must be a `DYN_CLOSURE` value.
pub(crate) unsafe fn call_with(callee: LkDyn, args: &mut Vec<LkDyn>) -> LkDyn {
    // SAFETY: as documented.
    let closure = unsafe { closure_of(callee) };
    if args.len() as i64 != closure.params {
        crate::panic::raise_str("closure called with the wrong number of arguments");
    }
    // The environment follows the visible arguments, which is the order the
    // native signature declares (`lower_call`'s hidden trailing captures).
    args.extend(closure.env.iter().map(materialize));
    let code = closure.code;
    match args.len() {
        0 => call0(code),
        1 => call1(code, args[0]),
        2 => call2(code, args[0], args[1]),
        3 => call3(code, args[0], args[1], args[2]),
        4 => call4(code, args[0], args[1], args[2], args[3]),
        5 => call5(code, args[0], args[1], args[2], args[3], args[4]),
        6 => call6(code, args[0], args[1], args[2], args[3], args[4], args[5]),
        7 => call7(code, args[0], args[1], args[2], args[3], args[4], args[5], args[6]),
        8 => call8(
            code, args[0], args[1], args[2], args[3], args[4], args[5], args[6], args[7],
        ),
        _ => crate::panic::raise_str("closure arity over the native cap"),
    }
}

/// `m.thing(args…)` where `thing` is a map entry or a struct field holding a
/// callable — the interpreter's callable-property path.
///
/// Its own entry point rather than an argument to [`lkrt_closure_call`] so the
/// *miss* can say what the interpreter says. A map is the one receiver where a
/// miss has two causes, and the interpreter names both; answering "value is not
/// callable" instead would be a different string out of the same `catch`.
///
/// # Safety
/// `args_block` as [`lkrt_closure_call`]; `name` a NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_closure_call_property(
    property: LkDyn,
    args_block: *mut c_void,
    name: *const core::ffi::c_char,
) -> LkDyn {
    if property.tag != DYN_CLOSURE {
        // SAFETY: `name` is a NUL-terminated constant from the module's pool.
        let method = unsafe { core::ffi::CStr::from_ptr(name) }.to_string_lossy();
        crate::panic::raise_str(&alloc::format!(
            "a Map has no method `{method}`, and this map has no key `{method}` holding a function either"
        ));
    }
    // SAFETY: the tag is checked above; the block contract is the callee's.
    unsafe { lkrt_closure_call(property, args_block) }
}

/// Deep-copies a closure value, for the boundaries that copy (a channel, a
/// goroutine's isolate). The code pointer is shared — it is code.
pub(crate) fn own_closure(value: LkDyn) -> OwnedVal {
    // SAFETY: the tag is only set by `lkrt_closure_new`.
    let closure = unsafe { closure_of(value) };
    OwnedVal::Closure(
        closure.code as usize,
        closure.params,
        closure.fn_index,
        closure.env.clone(),
    )
}

/// The inverse: a fresh handle in the current arena.
pub(crate) fn materialize_closure(code: usize, params: i64, fn_index: i64, env: &[OwnedVal]) -> LkDyn {
    LkDyn {
        tag: DYN_CLOSURE,
        payload: crate::state::arena_handle(LkClosure {
            code: code as *const c_void,
            params,
            fn_index,
            env: env.to_vec(),
        }) as i64,
    }
}

/// What `display` prints — the interpreter's exact wording, index and all
/// (`runtime_display_callable`).
///
/// # Safety
/// `value` must be a `DYN_CLOSURE`.
pub(crate) unsafe fn closure_text(value: LkDyn) -> alloc::string::String {
    // SAFETY: as documented.
    let closure = unsafe { closure_of(value) };
    alloc::format!("<fn #{}({} captures)>", closure.fn_index, closure.env.len())
}

/// # Safety
/// `value` must be a `DYN_CLOSURE`.
unsafe fn closure_of<'a>(value: LkDyn) -> &'a LkClosure {
    if value.tag != DYN_CLOSURE {
        crate::panic::raise_str("value is not callable");
    }
    // SAFETY: the payload of a `DYN_CLOSURE` is an `LkClosure` handle.
    unsafe { &*(value.payload as *const LkClosure) }
}

/// One `extern "C"` signature per arity. A `LkDyn` is a two-word aggregate, so
/// there is no variadic call to make instead.
macro_rules! closure_arity {
    ($($name:ident($($arg:ident),*);)*) => {
        $(
            #[allow(clippy::too_many_arguments)]
            fn $name(code: *const c_void, $($arg: LkDyn),*) -> LkDyn {
                // SAFETY: every `lk_fn_N` a closure can name is a *clone* the
                // lowering made for exactly this purpose, with an all-`LkDyn`
                // signature of exactly this arity (`SigInfer::value_lambdas`).
                let f: extern "C" fn($(closure_arity!(@ty $arg)),*) -> LkDyn =
                    unsafe { core::mem::transmute(code) };
                f($($arg),*)
            }
        )*
    };
    (@ty $arg:ident) => { LkDyn };
}

closure_arity! {
    call0();
    call1(a0);
    call2(a0, a1);
    call3(a0, a1, a2);
    call4(a0, a1, a2, a3);
    call5(a0, a1, a2, a3, a4);
    call6(a0, a1, a2, a3, a4, a5);
    call7(a0, a1, a2, a3, a4, a5, a6);
    call8(a0, a1, a2, a3, a4, a5, a6, a7);
}
