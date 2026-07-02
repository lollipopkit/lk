use crate::{
    abi::{aborting, c_str, owned_c_string, status, write_out},
    state::with_runtime,
};
use std::ffi::c_char;
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
        .unwrap_or(std::ptr::null_mut());
        write_out(out, value, "env.get")
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn lkrt_env_get_or(key: *const c_char, default: *const c_char) -> *mut c_char {
    aborting(|| {
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
    aborting(|| {
        let key = c_str(key, "env.has key")?;
        let _env = env_lock();
        Ok(i64::from(std::env::var_os(key.as_str()).is_some()))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn lkrt_env_set(key: *const c_char, value: *const c_char) -> i64 {
    status(|| {
        let key = c_str(key, "env.set key")?;
        let value = c_str(value, "env.set value")?;
        let _env = env_lock();
        // SAFETY: Rust 2024 requires process environment reads and writes to
        // be serialized. Every lkrt env accessor takes this process-wide mutex
        // before touching std::env, including reads and mutations.
        unsafe {
            std::env::set_var(key, value);
        }
        Ok(())
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn lkrt_env_remove(key: *const c_char) -> i64 {
    status(|| {
        let key = c_str(key, "env.remove key")?;
        let _env = env_lock();
        // SAFETY: See lkrt_env_set; all lkrt std::env access is serialized.
        unsafe {
            std::env::remove_var(key);
        }
        Ok(())
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn lkrt_fs_exists(path: *const c_char) -> i64 {
    aborting(|| {
        let path = c_str(path, "fs.exists path")?;
        Ok(i64::from(Path::new(path.as_str()).exists()))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn lkrt_fs_read(path: *const c_char) -> i64 {
    aborting(|| {
        let path = c_str(path, "fs.read path")?;
        let data = fs::read(path.as_str()).map_err(|err| format!("fs.read {path}: {err}"))?;
        Ok(with_runtime(|rt| rt.insert_bytes(data)))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn lkrt_fs_read_to_string(path: *const c_char) -> *mut c_char {
    aborting(|| {
        let path = c_str(path, "fs.read_to_string path")?;
        let data = fs::read_to_string(path.as_str()).map_err(|err| format!("fs.read_to_string {path}: {err}"))?;
        owned_c_string(data)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn lkrt_fs_write_str(path: *const c_char, data: *const c_char) -> i64 {
    aborting(|| {
        let path = c_str(path, "fs.write path")?;
        let data = c_str(data, "fs.write data")?;
        fs::write(path.as_str(), data.as_bytes()).map_err(|err| format!("fs.write {path}: {err}"))?;
        Ok(1)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn lkrt_fs_write_bytes(path: *const c_char, data: i64) -> i64 {
    aborting(|| {
        let path = c_str(path, "fs.write path")?;
        let data = with_runtime(|rt| rt.take_bytes(data))?;
        fs::write(path.as_str(), &data).map_err(|err| format!("fs.write {path}: {err}"))?;
        Ok(1)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn lkrt_fs_read_dir(path: *const c_char) -> i64 {
    aborting(|| {
        let path = c_str(path, "fs.read_dir path")?;
        let count = fs::read_dir(path.as_str())
            .map_err(|err| format!("fs.read_dir {path}: {err}"))?
            .count();
        Ok(count as i64)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn lkrt_fs_metadata_len(path: *const c_char) -> i64 {
    aborting(|| fs_metadata_field(path, MetadataField::Len))
}

#[unsafe(no_mangle)]
pub extern "C" fn lkrt_fs_metadata_is_file(path: *const c_char) -> i64 {
    aborting(|| fs_metadata_field(path, MetadataField::IsFile))
}

#[unsafe(no_mangle)]
pub extern "C" fn lkrt_fs_metadata_is_dir(path: *const c_char) -> i64 {
    aborting(|| fs_metadata_field(path, MetadataField::IsDir))
}

#[unsafe(no_mangle)]
pub extern "C" fn lkrt_fs_metadata_readonly(path: *const c_char) -> i64 {
    aborting(|| fs_metadata_field(path, MetadataField::Readonly))
}

#[unsafe(no_mangle)]
pub extern "C" fn lkrt_fs_canonicalize(path: *const c_char) -> *mut c_char {
    aborting(|| {
        let path = c_str(path, "fs.canonicalize path")?;
        let path = fs::canonicalize(path.as_str()).map_err(|err| format!("fs.canonicalize {path}: {err}"))?;
        owned_c_string(path.to_string_lossy())
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn lkrt_fs_temp_dir() -> *mut c_char {
    aborting(|| owned_c_string(std::env::temp_dir().to_string_lossy()))
}

#[unsafe(no_mangle)]
pub extern "C" fn lkrt_path_temp_dir() -> *mut c_char {
    lkrt_fs_temp_dir()
}

#[unsafe(no_mangle)]
pub extern "C" fn lkrt_process_cwd() -> *mut c_char {
    aborting(|| {
        let cwd = std::env::current_dir().map_err(|err| format!("process.cwd failed: {err}"))?;
        owned_c_string(cwd.to_string_lossy())
    })
}

/// `math.floor(Float)` with the VM's exact semantics: `value.floor() as i64`
/// (a saturating cast, matching `integer_round` in the stdlib math module).
/// `Int` arguments never reach here — the lowering passes them through.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_math_floor(value: f64) -> i64 {
    value.floor() as i64
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

#[unsafe(no_mangle)]
pub extern "C" fn lkrt_time_now_ms() -> i64 {
    epoch_millis()
}

#[unsafe(no_mangle)]
pub extern "C" fn lkrt_time_sleep_ms(ms: i64) {
    aborting(|| {
        if ms < 0 {
            return Err(format!("time.sleep expects non-negative milliseconds, got {ms}"));
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
    use crate::{lkrt_bytes_free, lkrt_string_free};
    use std::ffi::{CStr, CString};

    #[test]
    fn env_get_reports_absent_value_without_string_handle() {
        let key = CString::new(format!("LKRT_TEST_MISSING_{}", std::process::id())).expect("key");
        let mut out = std::ptr::null_mut();

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
        assert_eq!(lkrt_fs_read_dir(dir.as_ptr()), 1);
        assert_eq!(lkrt_fs_metadata_len(file.as_ptr()), 5);
        assert_eq!(lkrt_fs_metadata_is_file(file.as_ptr()), 1);
        assert_eq!(lkrt_fs_metadata_is_dir(file.as_ptr()), 0);

        let bytes = lkrt_fs_read(file.as_ptr());
        assert!(bytes > 0);
        let text_ptr = crate::lkrt_bytes_to_string_utf8(bytes);
        assert!(!text_ptr.is_null());
        // SAFETY: text_ptr is an lkrt-owned NUL-terminated CString pointer.
        let text = unsafe { CStr::from_ptr(text_ptr) };
        assert_eq!(text.to_str().expect("utf8"), "hello");
        // SAFETY: frees the original owned pointer. A pointer re-derived via
        // `&CStr::as_ptr().cast_mut()` only carries shared read-only
        // provenance, so handing it to `CString::from_raw` is UB (caught by
        // Miri's Stacked Borrows checking).
        unsafe { lkrt_string_free(text_ptr) };
        assert_eq!(lkrt_bytes_free(bytes), 0);

        let canonical = lkrt_fs_canonicalize(file.as_ptr());
        assert!(!canonical.is_null());
        // SAFETY: the pointer came from an lkrt owned-string return.
        unsafe { lkrt_string_free(canonical) };
    }
}
