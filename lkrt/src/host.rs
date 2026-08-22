use crate::abi::{c_str, owned_c_string, raising, status, write_out};
use core::ffi::c_char;
use std::{
    fs,
    path::Path,
    sync::{Mutex, MutexGuard, OnceLock},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

#[derive(Clone, Copy)]
enum MetadataField {
    Len,
    IsFile,
    IsDir,
    Readonly,
}

#[unsafe(no_mangle)]
pub extern "C" fn lkrt_env_get(key: *const c_char, out: *mut *mut c_char) -> i64 {
    status(|| {
        let key = c_str(key, "env.get key")?;
        let value = {
            let _env = env_lock();
            std::env::var_os(key.as_str()).and_then(|value| value.into_string().ok())
        }
        .map(owned_c_string)
        .transpose()?
        .unwrap_or(core::ptr::null_mut());
        write_out(out, value, "env.get")
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn lkrt_env_get_or(key: *const c_char, default: *const c_char) -> *mut c_char {
    raising(|| {
        let key = c_str(key, "env.get_or key")?;
        let default = c_str(default, "env.get_or default")?;
        let value = {
            let _env = env_lock();
            std::env::var(key.as_str()).unwrap_or(default)
        };
        owned_c_string(value)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn lkrt_env_has(key: *const c_char) -> i64 {
    raising(|| {
        let key = c_str(key, "env.has key")?;
        let _env = env_lock();
        Ok(i64::from(std::env::var_os(key.as_str()).is_some()))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn lkrt_fs_exists(path: *const c_char) -> i64 {
    raising(|| {
        let path = c_str(path, "fs.exists path")?;
        Ok(i64::from(Path::new(path.as_str()).exists()))
    })
}

/// `fs.is_file(path)` / `fs.is_dir(path)` — `Path::is_file`/`is_dir`, which
/// answer false rather than raising for a path that does not exist.
/// `fs.metadata(path)` — the stdlib module's four-key map, built through the
/// VM's own two-stage construction so it *iterates* the same way.
///
/// The keys go in in the module's order (`len`, `is_file`, `is_dir`,
/// `readonly`) and `str_dyn_map_mirrored` replays the same rehash the VM does,
/// because a map's iteration order is what `println` prints.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_fs_metadata_map(path: *const c_char) -> *mut core::ffi::c_void {
    raising(|| {
        let path = c_str(path, "fs.metadata path")?;
        let meta = fs::metadata(path.as_str()).map_err(|err| format!("failed to stat '{path}': {err}"))?;
        let pairs = alloc::vec![
            (
                alloc::string::String::from("len"),
                crate::lkdyn::lkrt_dyn_from_i64(meta.len() as i64)
            ),
            (
                alloc::string::String::from("is_file"),
                crate::lkdyn::lkrt_dyn_from_bool(i64::from(meta.is_file()))
            ),
            (
                alloc::string::String::from("is_dir"),
                crate::lkdyn::lkrt_dyn_from_bool(i64::from(meta.is_dir()))
            ),
            (
                alloc::string::String::from("readonly"),
                crate::lkdyn::lkrt_dyn_from_bool(i64::from(meta.permissions().readonly()))
            ),
        ];
        Ok(crate::vm_mirror::str_dyn_map_mirrored(pairs))
    })
}

/// `env.vars()` — every environment variable, in `std::env::vars_os` order,
/// through the same mirrored construction.
///
/// Lossy conversion on both sides: the stdlib module calls `to_string_lossy` on
/// key and value, so a non-UTF-8 variable is U+FFFD there and here.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_env_vars_map() -> *mut core::ffi::c_void {
    raising(|| {
        let mut pairs = Vec::new();
        {
            let _env = env_lock();
            for (key, value) in std::env::vars_os() {
                let key = key.to_string_lossy().into_owned();
                let value = value.to_string_lossy().into_owned();
                let value = owned_c_string(value)?;
                pairs.push((key, crate::lkdyn::lkrt_dyn_from_str(value)));
            }
        }
        Ok(crate::vm_mirror::str_dyn_map_mirrored(pairs))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn lkrt_fs_is_file(path: *const c_char) -> i64 {
    raising(|| {
        let path = c_str(path, "fs.is_file path")?;
        Ok(i64::from(Path::new(path.as_str()).is_file()))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn lkrt_fs_is_dir(path: *const c_char) -> i64 {
    raising(|| {
        let path = c_str(path, "fs.is_dir path")?;
        Ok(i64::from(Path::new(path.as_str()).is_dir()))
    })
}

/// `fs.append(path, text)` — creates the file if it is absent, like the stdlib
/// module's `OpenOptions::new().create(true).append(true)`.
///
/// Two error messages, not one: the stdlib distinguishes failing to *open* from
/// failing to *write*, and both are program-visible text.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_fs_append_str(path: *const c_char, data: *const c_char) -> i64 {
    raising(|| {
        let path = c_str(path, "fs.append path")?;
        let data = c_str(data, "fs.append data")?;
        append_bytes(path.as_str(), data.as_bytes())
    })
}

/// `fs.append(path, bytes)`.
///
/// # Safety
/// `data` must be a live `Bytes` handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_fs_append_bytes(path: *const c_char, data: *mut core::ffi::c_void) -> i64 {
    raising(|| {
        let path = c_str(path, "fs.append path")?;
        let data = crate::lkbytes::bytes_slice(data).to_vec();
        append_bytes(path.as_str(), &data)
    })
}

fn append_bytes(path: &str, data: &[u8]) -> Result<i64, alloc::string::String> {
    use std::io::Write as _;
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|err| format!("failed to open file '{path}': {err}"))?;
    file.write_all(data)
        .map_err(|err| format!("failed to append file '{path}': {err}"))?;
    Ok(1)
}

/// `fs.create_dir(path)` — one level, and `fs.create_dir_all(path)`, the whole
/// chain. Both report with the stdlib module's single wording.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_fs_create_dir(path: *const c_char) -> i64 {
    raising(|| {
        let path = c_str(path, "fs.create_dir path")?;
        fs::create_dir(path.as_str()).map_err(|err| format!("failed to create directory '{path}': {err}"))?;
        Ok(1)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn lkrt_fs_create_dir_all(path: *const c_char) -> i64 {
    raising(|| {
        let path = c_str(path, "fs.create_dir_all path")?;
        fs::create_dir_all(path.as_str()).map_err(|err| format!("failed to create directory '{path}': {err}"))?;
        Ok(1)
    })
}

/// The three `fs.remove_*` members.
///
/// **A missing path is `false`, not a raise** — the stdlib module's
/// `remove_path` singles out `NotFound` and every other error raises `failed to
/// remove '{path}'`. Writing the raise for all of them would have turned a
/// two-valued answer into a control-flow difference between the back ends.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_fs_remove_file(path: *const c_char) -> i64 {
    raising(|| {
        let path = c_str(path, "fs.remove_file path")?;
        remove_result(path.as_str(), fs::remove_file(path.as_str()))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn lkrt_fs_remove_dir(path: *const c_char) -> i64 {
    raising(|| {
        let path = c_str(path, "fs.remove_dir path")?;
        remove_result(path.as_str(), fs::remove_dir(path.as_str()))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn lkrt_fs_remove_dir_all(path: *const c_char) -> i64 {
    raising(|| {
        let path = c_str(path, "fs.remove_dir_all path")?;
        remove_result(path.as_str(), fs::remove_dir_all(path.as_str()))
    })
}

fn remove_result(path: &str, result: std::io::Result<()>) -> Result<i64, alloc::string::String> {
    match result {
        Ok(()) => Ok(1),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(0),
        Err(err) => Err(format!("failed to remove '{path}': {err}")),
    }
}

/// `fs.rename(from, to)` — the error names *from*, as the stdlib module does.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_fs_rename(from: *const c_char, to: *const c_char) -> i64 {
    raising(|| {
        let from = c_str(from, "fs.rename from")?;
        let to = c_str(to, "fs.rename to")?;
        fs::rename(from.as_str(), to.as_str()).map_err(|err| format!("failed to rename '{from}': {err}"))?;
        Ok(1)
    })
}

/// `fs.copy(from, to)` — answers the byte count, not a bool.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_fs_copy(from: *const c_char, to: *const c_char) -> i64 {
    raising(|| {
        let from = c_str(from, "fs.copy from")?;
        let to = c_str(to, "fs.copy to")?;
        let copied = fs::copy(from.as_str(), to.as_str()).map_err(|err| format!("failed to copy '{from}': {err}"))?;
        Ok(copied as i64)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn lkrt_fs_read(path: *const c_char) -> *mut core::ffi::c_void {
    raising(|| {
        let path = c_str(path, "fs.read path")?;
        let data = fs::read(path.as_str()).map_err(|err| format!("fs.read {path}: {err}"))?;
        Ok(crate::lkbytes::bytes_handle(data))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn lkrt_fs_read_to_string(path: *const c_char) -> *mut c_char {
    raising(|| {
        let path = c_str(path, "fs.read_to_string path")?;
        let data = fs::read_to_string(path.as_str()).map_err(|err| format!("failed to read file '{path}': {err}"))?;
        owned_c_string(data)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn lkrt_fs_write_str(path: *const c_char, data: *const c_char) -> i64 {
    raising(|| {
        let path = c_str(path, "fs.write path")?;
        let data = c_str(data, "fs.write data")?;
        fs::write(path.as_str(), data.as_bytes()).map_err(|err| format!("failed to write file '{path}': {err}"))?;
        Ok(1)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn lkrt_fs_write_bytes(path: *const c_char, data: *mut core::ffi::c_void) -> i64 {
    raising(|| {
        let path = c_str(path, "fs.write path")?;
        let data = crate::lkbytes::bytes_slice(data).to_vec();
        fs::write(path.as_str(), &data).map_err(|err| format!("failed to write file '{path}': {err}"))?;
        Ok(1)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn lkrt_fs_metadata_len(path: *const c_char) -> i64 {
    raising(|| fs_metadata_field(path, MetadataField::Len))
}

#[unsafe(no_mangle)]
pub extern "C" fn lkrt_fs_metadata_is_file(path: *const c_char) -> i64 {
    raising(|| fs_metadata_field(path, MetadataField::IsFile))
}

#[unsafe(no_mangle)]
pub extern "C" fn lkrt_fs_metadata_is_dir(path: *const c_char) -> i64 {
    raising(|| fs_metadata_field(path, MetadataField::IsDir))
}

#[unsafe(no_mangle)]
pub extern "C" fn lkrt_fs_metadata_readonly(path: *const c_char) -> i64 {
    raising(|| fs_metadata_field(path, MetadataField::Readonly))
}

/// `fs.canonicalize(path)` — the resolved path, or **nil** when it is not
/// UTF-8.
///
/// The nil is the language's answer (`returns = String?`), not a convenience:
/// canonicalizing follows symlinks, and a Linux path component is arbitrary
/// bytes, so a resolved path that no LK string can hold is reachable. This used
/// to hand back `to_string_lossy`, which invents U+FFFD where the VM answers
/// nil — a different value, not a different rendering.
///
/// (`fs.temp_dir` is also `String?` and stays a plain string: its path comes
/// from the OS's own temp-directory setting, so the same nil is not reachable
/// through it in the way a user-supplied path is.)
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_fs_canonicalize(path: *const c_char) -> crate::lkdyn::LkDyn {
    raising(|| {
        let path = c_str(path, "fs.canonicalize path")?;
        let resolved =
            fs::canonicalize(path.as_str()).map_err(|err| format!("failed to canonicalize '{path}': {err}"))?;
        Ok(match resolved.into_os_string().into_string() {
            Ok(text) => crate::lkdyn::lkrt_dyn_from_str(owned_c_string(text)?),
            Err(_) => crate::lkdyn::lkrt_dyn_from_nil(),
        })
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn lkrt_fs_temp_dir() -> *mut c_char {
    raising(|| owned_c_string(std::env::temp_dir().to_string_lossy()))
}

#[unsafe(no_mangle)]
pub extern "C" fn lkrt_path_temp_dir() -> *mut c_char {
    lkrt_fs_temp_dir()
}

#[unsafe(no_mangle)]
pub extern "C" fn lkrt_process_cwd() -> *mut c_char {
    raising(|| {
        let cwd = std::env::current_dir().map_err(|err| format!("process.cwd failed: {err}"))?;
        owned_c_string(cwd.to_string_lossy())
    })
}

/// `os.hostname()` — `HOSTNAME`/`COMPUTERNAME` env var or `localhost`, the
/// stdlib os module's exact fallback chain.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_os_hostname() -> *mut c_char {
    raising(|| {
        let hostname = std::env::var_os("HOSTNAME")
            .or_else(|| std::env::var_os("COMPUTERNAME"))
            .and_then(|value| value.into_string().ok())
            .unwrap_or_else(|| "localhost".to_string());
        owned_c_string(hostname)
    })
}

/// `os.arch()` — `std::env::consts::ARCH` (identical to the VM: lkrt compiles
/// for the same target the interpreter runs on).
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_os_arch() -> *mut c_char {
    raising(|| owned_c_string(std::env::consts::ARCH))
}

/// `os.os()` — `std::env::consts::OS`.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_os_name() -> *mut c_char {
    raising(|| owned_c_string(std::env::consts::OS))
}

/// `fs.read_dir(path)` — the sorted list of entry *names* (UTF-8 names only,
/// the VM's `to_str` filter) as a `List<str>` handle; IO errors abort loudly
/// (the VM's error is equally fatal).
///
/// # Safety
/// `path` must be a valid, non-null NUL-terminated C string (a null pointer
/// is a caller bug and aborts with a loud `lkrt error`).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_fs_read_dir_list(path: *const c_char) -> *mut core::ffi::c_void {
    raising(|| {
        let path = c_str(path, "fs.read_dir path")?;
        let mut names = Vec::new();
        for entry in fs::read_dir(path.as_str()).map_err(|err| format!("failed to read directory '{path}': {err}"))? {
            let entry = entry.map_err(|err| format!("failed to read directory entry '{path}': {err}"))?;
            if let Some(name) = entry.file_name().to_str() {
                names.push(name.to_string());
            }
        }
        names.sort();
        let mut list: Vec<*const core::ffi::c_char> = Vec::with_capacity(names.len());
        for name in names {
            list.push(owned_c_string(name)?.cast_const());
        }
        Ok(crate::state::arena_handle(list))
    })
}

/// `math.floor(Float)` with the VM's exact semantics: `value.floor() as i64`
/// (a saturating cast, matching `integer_round` in the stdlib math module).
/// `Int` arguments never reach here — the lowering passes them through.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_math_floor(value: f64) -> i64 {
    value.floor() as i64
}

/// `math.ceil(Float)` — `integer_round` with `f64::ceil` (see [`lkrt_math_floor`]).
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_math_ceil(value: f64) -> i64 {
    value.ceil() as i64
}

/// `math.round(Float)` — `integer_round` with `f64::round` (see [`lkrt_math_floor`]).
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_math_round(value: f64) -> i64 {
    value.round() as i64
}

/// `math.sqrt(Number)` — the stdlib module rejects negative arguments loudly,
/// so the guard raises (matching the VM's error), never returns NaN.
///
/// The message goes to `raise_str`, not to stderr. It used to do both, the wrong
/// way round: the real reason was printed and `"runtime error"` was raised, so
/// `try { math.sqrt(-1.0) } catch e { e }` evaluated to `"runtime error"`
/// compiled and to `"sqrt() argument must be non-negative"` interpreted — and a
/// caught error's text *is* the program's output.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_math_sqrt(value: f64) -> f64 {
    if value < 0.0 {
        crate::panic::raise_str("sqrt() argument must be non-negative");
    }
    value.sqrt()
}

/// `math.sin(Number)` → Float.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_math_sin(value: f64) -> f64 {
    value.sin()
}

/// `math.cos(Number)` → Float.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_math_cos(value: f64) -> f64 {
    value.cos()
}

/// `math.tan(Number)` → Float.
///
/// `sin` and `cos` were native and `tan` was not — the same class of function,
/// split for no reason a program can see.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_math_tan(value: f64) -> f64 {
    value.tan()
}

/// The domain guards the stdlib module states, raising *its* words.
macro_rules! math_domain {
    ($name:ident, $call:ident, $ok:expr, $message:literal, $doc:literal) => {
        #[doc = $doc]
        #[unsafe(no_mangle)]
        pub extern "C" fn $name(value: f64) -> f64 {
            let ok: fn(f64) -> bool = $ok;
            if !ok(value) {
                crate::panic::raise_str($message);
            }
            value.$call()
        }
    };
}

math_domain!(
    lkrt_math_asin,
    asin,
    |v| (-1.0..=1.0).contains(&v),
    "asin() argument must be between -1 and 1",
    "`math.asin(Number)` → Float; outside `-1..=1` is the module's loud error."
);
math_domain!(
    lkrt_math_acos,
    acos,
    |v| (-1.0..=1.0).contains(&v),
    "acos() argument must be between -1 and 1",
    "`math.acos(Number)` → Float; outside `-1..=1` is the module's loud error."
);
math_domain!(
    lkrt_math_log,
    ln,
    |v| v > 0.0,
    "log() argument must be positive",
    "`math.log(Number)` → natural log; a non-positive argument raises."
);
math_domain!(
    lkrt_math_log10,
    log10,
    |v| v > 0.0,
    "log10() argument must be positive",
    "`math.log10(Number)` → Float; a non-positive argument raises."
);
math_domain!(
    lkrt_math_log2,
    log2,
    |v| v > 0.0,
    "log2() argument must be positive",
    "`math.log2(Number)` → Float; a non-positive argument raises."
);

/// `math.atan(Number)` → Float. Total, so no guard.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_math_atan(value: f64) -> f64 {
    value.atan()
}

/// `math.atan2(y, x)` → Float. Total, including `atan2(0, 0)`.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_math_atan2(y: f64, x: f64) -> f64 {
    y.atan2(x)
}

/// `math.clamp(value, low, high)` on `Int` — the module's only arity.
///
/// The module rejects an inverted range loudly rather than picking a side.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_math_clamp_i64(value: i64, low: i64, high: i64) -> i64 {
    if low > high {
        crate::panic::raise_str("clamp() requires 'min' to be less than or equal to 'max'");
    }
    value.clamp(low, high)
}

/// `math.exp(Number)` → Float.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_math_exp(value: f64) -> f64 {
    value.exp()
}

/// `math.pow(base, exponent)` → Float (`f64::powf`, both args f64-promoted).
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_math_pow(base: f64, exponent: f64) -> f64 {
    base.powf(exponent)
}

/// `math.hypot(x, y)` → Float (`f64::hypot`).
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_math_hypot(x: f64, y: f64) -> f64 {
    x.hypot(y)
}

/// `math.cbrt(x)` → Float (`f64::cbrt`).
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_math_cbrt(x: f64) -> f64 {
    x.cbrt()
}

/// `math.sinh(Number)` → Float.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_math_sinh(value: f64) -> f64 {
    value.sinh()
}

/// `math.cosh(Number)` → Float.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_math_cosh(value: f64) -> f64 {
    value.cosh()
}

/// `math.tanh(Number)` → Float.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_math_tanh(value: f64) -> f64 {
    value.tanh()
}

/// `math.trunc(Float)` → Float. The module's Int arm hands the Int back
/// unchanged, so only the Float half is a call.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_math_trunc_f64(value: f64) -> f64 {
    value.trunc()
}

/// `math.fract(Float)` → Float. The Int arm answers `0.0` without a call.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_math_fract_f64(value: f64) -> f64 {
    value.fract()
}

/// `math.to_int(Float)` → Int, the module's `value as i64`.
///
/// A call rather than a MIR cast because Rust's `as` saturates and Cranelift's
/// `fcvt_to_sint` traps: `math.to_int(1e30)` is `i64::MAX` in the VM and would
/// have aborted the native build.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_math_to_int_f64(value: f64) -> i64 {
    value as i64
}

/// `math.is_inf(x)` → 0/1. Only a Float is ever infinite in the VM; an Int
/// argument promotes to a finite `f64` and answers false, same as the module.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_math_is_inf(x: f64) -> i64 {
    i64::from(x.is_infinite())
}

/// `math.is_nan(x)` → 0/1 (only a Float NaN is true in the VM; the lowering
/// promotes Int args, whose result is always false — same as the module).
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_math_is_nan(x: f64) -> i64 {
    i64::from(x.is_nan())
}

/// `math.sign(Int)` → `i64::signum` (the module's Int arm).
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_math_sign_i64(v: i64) -> i64 {
    v.signum()
}

/// `math.sign(Float)` → ±1.0/0.0 (the module's Float arm: NaN → 0.0 via the
/// final else, exactly the stdlib comparison chain).
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_math_sign_f64(v: f64) -> f64 {
    if v > 0.0 {
        1.0
    } else if v < 0.0 {
        -1.0
    } else {
        0.0
    }
}

/// `path.sep()` — the platform's main separator (the stdlib module's
/// `MAIN_SEPARATOR_STR`).
/// The `path` module's `String?` parts, each one `std::path`'s own answer.
///
/// The stdlib module calls exactly these `Path` methods, so sharing the
/// *underlying crate* is what keeps the two ends identical — the same discipline
/// the base64/hex and datetime helpers here follow, rather than a second
/// implementation of the rule.
macro_rules! path_part {
    ($name:ident, $call:ident, $doc:literal) => {
        #[doc = $doc]
        /// # Safety
        /// `path` must be a valid C string.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $name(path: *const c_char) -> crate::lkdyn::LkDyn {
            // SAFETY: caller guarantees a valid C string.
            let text = unsafe { core::ffi::CStr::from_ptr(path) }.to_str().unwrap_or("");
            match std::path::Path::new(text).$call() {
                Some(part) => {
                    let owned = crate::lkstr::arena_c_string(
                        alloc::ffi::CString::new(part.to_string_lossy().as_ref()).unwrap_or_default(),
                    );
                    crate::lkdyn::lkrt_dyn_from_str(owned)
                }
                None => crate::lkdyn::LkDyn::NIL,
            }
        }
    };
}

/// `path.normalize(p)` → String — the module's component walk, verbatim.
///
/// `..` cancels only a *named* component, so `normalize("../..")` keeps both;
/// above a root it means nothing and is dropped, because `/..` is `/` on every
/// filesystem and keeping it produces a path that normalizes to itself forever.
///
/// # Safety
/// `path` must be a valid C string, or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_path_normalize(path: *const c_char) -> *mut c_char {
    use std::path::Component;
    // SAFETY: the caller guarantees a valid C string.
    let text = unsafe { core::ffi::CStr::from_ptr(path) }.to_str().unwrap_or("");
    let path = std::path::Path::new(text);
    let rooted = path.has_root();
    let mut out = std::path::PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if matches!(out.components().next_back(), Some(Component::Normal(_))) {
                    out.pop();
                } else if !rooted {
                    out.push(component.as_os_str());
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    crate::lkstr::arena_c_string(alloc::ffi::CString::new(out.to_string_lossy().as_ref()).unwrap_or_default())
}

path_part!(lkrt_path_parent, parent, "`path.parent(p)` → String?");
path_part!(lkrt_path_file_name, file_name, "`path.file_name(p)` → String?");
path_part!(lkrt_path_file_stem, file_stem, "`path.file_stem(p)` → String?");
path_part!(lkrt_path_extension, extension, "`path.extension(p)` → String?");

/// `path.with_extension(p, ext)` → String.
///
/// # Safety
/// Both arguments must be valid C strings.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_path_with_extension(path: *const c_char, ext: *const c_char) -> *mut c_char {
    // SAFETY: caller guarantees valid C strings.
    let (text, ext) = unsafe {
        (
            core::ffi::CStr::from_ptr(path).to_str().unwrap_or(""),
            core::ffi::CStr::from_ptr(ext).to_str().unwrap_or(""),
        )
    };
    let joined = std::path::Path::new(text).with_extension(ext);
    crate::lkstr::arena_c_string(alloc::ffi::CString::new(joined.to_string_lossy().as_ref()).unwrap_or_default())
}

/// `path.is_absolute(p)` → Bool, as 1/0.
///
/// # Safety
/// `path` must be a valid C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_path_is_absolute(path: *const c_char) -> i64 {
    // SAFETY: caller guarantees a valid C string.
    let text = unsafe { core::ffi::CStr::from_ptr(path) }.to_str().unwrap_or("");
    i64::from(std::path::Path::new(text).is_absolute())
}

/// `path.components(p)` → List<String>, the same `Path::components` walk.
///
/// # Safety
/// `path` must be a valid C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_path_components(path: *const c_char) -> *mut core::ffi::c_void {
    // SAFETY: caller guarantees a valid C string.
    let text = unsafe { core::ffi::CStr::from_ptr(path) }.to_str().unwrap_or("");
    let parts: Vec<*const c_char> = std::path::Path::new(text)
        .components()
        .map(|component| {
            crate::lkstr::arena_c_string(
                alloc::ffi::CString::new(component.as_os_str().to_string_lossy().as_ref()).unwrap_or_default(),
            )
            .cast_const()
        })
        .collect();
    crate::state::arena_handle(parts)
}

/// The stdlib `path` module's `delimiter`: the character that separates
/// *entries* in a `PATH`-style variable, as opposed to `sep`, which separates
/// components within one path.
///
/// `std::path` names only the latter (`MAIN_SEPARATOR_STR`), so the platform
/// answer is written out here — the same two-line `cfg!(windows)` the stdlib
/// module has. That is a rule in two places, which is why the table test in
/// `aot/lower/src/tables.rs` names it: it exists so the pair cannot silently
/// disagree the day one of them learns a third platform.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_path_delimiter() -> *mut c_char {
    let delimiter = if cfg!(windows) { ";" } else { ":" };
    crate::lkstr::arena_c_string(alloc::ffi::CString::new(delimiter).unwrap_or_default())
}

#[unsafe(no_mangle)]
pub extern "C" fn lkrt_path_sep() -> *mut c_char {
    crate::lkstr::arena_c_string(alloc::ffi::CString::new(std::path::MAIN_SEPARATOR_STR).unwrap_or_default())
}

/// The stdlib datetime module's `utc_datetime`: raises on an out-of-range
/// timestamp, with the module's own words.
///
/// `invalid timestamp` and nothing else — the same reason the caught value has
/// to be the message: this used to print `lkrt error: {context}: invalid
/// timestamp` and raise `"runtime error"`, so a catching program saw neither the
/// reason nor the same text the VM gives it.
fn datetime_utc(timestamp: i64) -> chrono::DateTime<chrono::Utc> {
    match chrono::DateTime::<chrono::Utc>::from_timestamp(timestamp, 0) {
        Some(dt) => dt,
        None => crate::panic::raise_str("invalid timestamp"),
    }
}

/// `datetime.now()` — Unix epoch seconds (`chrono::Utc::now().timestamp()`).
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_datetime_now() -> i64 {
    chrono::Utc::now().timestamp()
}

/// `datetime.format(timestamp, format)` — chrono strftime formatting in UTC,
/// byte-identical to the stdlib module (same crate, same call).
///
/// # Safety
/// `format` must be a valid NUL-terminated C string, or null (empty).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_datetime_format(timestamp: i64, format: *const c_char) -> *mut c_char {
    raising(|| {
        let format = c_str(format, "datetime.format format")?;
        let formatted = datetime_utc(timestamp).format(format.as_str()).to_string();
        owned_c_string(formatted)
    })
}

/// `datetime.parse(value, format)` — anchored to UTC; a parse failure aborts
/// (the VM's loud error).
///
/// The three shapes `format` can write, tried in order: a full datetime, a
/// date alone (midnight, which is what the format dropped), a time alone (that
/// time on the epoch day). Only the first was here, so
/// `datetime.parse("2026-08-20", "%Y-%m-%d")` answered interpreted and failed
/// compiled — the stdlib module has said what the rule is above `parse_naive`
/// the whole time.
///
/// The refusal names the value and the format, for the reason written there:
/// chrono's own phrasing describes its parser's internal requirement ("input
/// is not enough for unique date and time"), a sentence about a library the
/// program never mentioned.
///
/// # Safety
/// Both pointers must be valid NUL-terminated C strings, or null (empty).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_datetime_parse(value: *const c_char, format: *const c_char) -> i64 {
    raising(|| {
        let value = c_str(value, "datetime.parse value")?;
        let format = c_str(format, "datetime.parse format")?;
        let naive = parse_naive(value.as_str(), format.as_str())
            .ok_or_else(|| format!("`{value}` does not match the format `{format}`"))?;
        Ok(chrono::DateTime::<chrono::Utc>::from_naive_utc_and_offset(naive, chrono::Utc).timestamp())
    })
}

/// `stdlib/crates/datetime`'s `parse_naive`, mirrored.
fn parse_naive(value: &str, format: &str) -> Option<chrono::NaiveDateTime> {
    if let Ok(naive) = chrono::NaiveDateTime::parse_from_str(value, format) {
        return Some(naive);
    }
    if let Ok(date) = chrono::NaiveDate::parse_from_str(value, format) {
        return Some(date.and_time(chrono::NaiveTime::MIN));
    }
    let time = chrono::NaiveTime::parse_from_str(value, format).ok()?;
    let epoch = chrono::DateTime::<chrono::Utc>::from_timestamp(0, 0)?.date_naive();
    Some(epoch.and_time(time))
}

/// `datetime.day_of_week(timestamp)` — the stdlib module's mapping (Sun = 0).
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_datetime_day_of_week(timestamp: i64) -> i64 {
    use chrono::Datelike;
    match datetime_utc(timestamp).weekday() {
        chrono::Weekday::Sun => 0,
        chrono::Weekday::Mon => 1,
        chrono::Weekday::Tue => 2,
        chrono::Weekday::Wed => 3,
        chrono::Weekday::Thu => 4,
        chrono::Weekday::Fri => 5,
        chrono::Weekday::Sat => 6,
    }
}

/// `datetime.day_of_year(timestamp)` — 1-based ordinal.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_datetime_day_of_year(timestamp: i64) -> i64 {
    use chrono::Datelike;
    i64::from(datetime_utc(timestamp).ordinal())
}

/// `datetime.is_weekend(timestamp)` — 1 for Sat/Sun, else 0 (the lowering
/// converts to the LK `Bool`).
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_datetime_is_weekend(timestamp: i64) -> i64 {
    use chrono::Datelike;
    i64::from(matches!(
        datetime_utc(timestamp).weekday(),
        chrono::Weekday::Sat | chrono::Weekday::Sun
    ))
}

#[unsafe(no_mangle)]
pub extern "C" fn lkrt_os_clock() -> f64 {
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_secs_f64()
}

#[unsafe(no_mangle)]
pub extern "C" fn lkrt_os_epoch() -> i64 {
    epoch_millis()
}

/// `os.time()` — Unix time in **seconds**, where `os.epoch()` is milliseconds.
/// Truncating the millisecond count would answer one second early for a
/// negative time, so this asks for seconds directly, as the module does.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_os_time() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

#[unsafe(no_mangle)]
pub extern "C" fn lkrt_time_now_ms() -> i64 {
    epoch_millis()
}

#[unsafe(no_mangle)]
pub extern "C" fn lkrt_time_sleep_ms(ms: i64) {
    raising(|| {
        if ms < 0 {
            // Same wording as the VM's `duration_millis`, because a caught
            // error's message *is* the program's output.
            return Err(format!(
                "time.sleep() expects a non-negative duration in milliseconds, got {ms}"
            ));
        }
        std::thread::sleep(Duration::from_millis(ms as u64));
        Ok(())
    })
}

fn epoch_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

fn fs_metadata_field(path: *const c_char, field: MetadataField) -> Result<i64, String> {
    let path = c_str(path, "fs.metadata path")?;
    let metadata = fs::metadata(path.as_str()).map_err(|err| format!("fs.metadata {path}: {err}"))?;
    let value = match field {
        MetadataField::Len => metadata.len() as i64,
        MetadataField::IsFile => i64::from(metadata.is_file()),
        MetadataField::IsDir => i64::from(metadata.is_dir()),
        MetadataField::Readonly => i64::from(metadata.permissions().readonly()),
    };
    Ok(value)
}

fn env_lock() -> MutexGuard<'static, ()> {
    static ENV_MUTEX: OnceLock<Mutex<()>> = OnceLock::new();
    ENV_MUTEX
        .get_or_init(|| Mutex::new(()))
        .lock()
        .expect("lkrt env mutex poisoned")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lkrt_string_free;
    use alloc::ffi::CString;
    use core::ffi::CStr;

    #[test]
    fn env_get_reports_absent_value_without_string_handle() {
        let key = CString::new(format!("LKRT_TEST_MISSING_{}", std::process::id())).expect("key");
        let mut out = core::ptr::null_mut();

        assert_eq!(lkrt_env_get(key.as_ptr(), &mut out), 0);
        assert!(out.is_null());
        assert_eq!(lkrt_env_has(key.as_ptr()), 0);
    }

    #[test]
    fn fs_helpers_return_owned_strings_and_typed_byte_handles() {
        let dir = std::env::temp_dir().join(format!("lkrt_host_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let file = dir.join("data.txt");
        std::fs::write(&file, b"hello").expect("write fixture");
        let file = CString::new(file.to_string_lossy().as_ref()).expect("file path");
        let dir = CString::new(dir.to_string_lossy().as_ref()).expect("dir path");

        assert_eq!(lkrt_fs_exists(file.as_ptr()), 1);
        let listing = unsafe { lkrt_fs_read_dir_list(dir.as_ptr()) };
        assert_eq!(unsafe { crate::lkrt_lklist_str_len(listing) }, 1);
        assert_eq!(lkrt_fs_metadata_len(file.as_ptr()), 5);
        assert_eq!(lkrt_fs_metadata_is_file(file.as_ptr()), 1);
        assert_eq!(lkrt_fs_metadata_is_dir(file.as_ptr()), 0);

        // A `Bytes` **value**: read its length, then its text, then its length
        // again — the one-shot host handle this used to be could only be read
        // once.
        let bytes = lkrt_fs_read(file.as_ptr());
        assert!(!bytes.is_null());
        assert_eq!(unsafe { crate::lkrt_lkbytes_len(bytes) }, 5);
        let text_ptr = unsafe { crate::lkrt_lkbytes_utf8(bytes) };
        assert!(!text_ptr.is_null());
        // SAFETY: text_ptr is an lkrt-owned NUL-terminated CString pointer.
        let text = unsafe { CStr::from_ptr(text_ptr) };
        assert_eq!(text.to_str().expect("utf8"), "hello");
        // SAFETY: frees the original owned pointer. A pointer re-derived via
        // `&CStr::as_ptr().cast_mut()` only carries shared read-only
        // provenance, so handing it to `CString::from_raw` is UB (caught by
        // Miri's Stacked Borrows checking).
        unsafe { lkrt_string_free(text_ptr) };
        assert_eq!(unsafe { crate::lkrt_lkbytes_len(bytes) }, 5);

        // `canonicalize` answers `String?`, so the result is boxed: a resolved
        // path that is not UTF-8 is nil, which is the VM's answer too.
        let canonical = lkrt_fs_canonicalize(file.as_ptr());
        assert_eq!(canonical.tag, crate::lkdyn::DYN_STR);
        // SAFETY: the payload came from an lkrt owned-string return.
        unsafe { lkrt_string_free(canonical.payload as *mut c_char) };
    }
}
