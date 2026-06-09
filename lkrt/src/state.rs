use std::{
    collections::{HashMap, HashSet},
    ffi::{CString, c_char},
    net::TcpStream,
    sync::{Mutex, OnceLock},
};

pub(crate) fn runtime() -> &'static Mutex<RuntimeState> {
    static RUNTIME: OnceLock<Mutex<RuntimeState>> = OnceLock::new();
    RUNTIME.get_or_init(|| Mutex::new(RuntimeState::default()))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HandleKind {
    Bytes,
    TcpStream,
}

#[derive(Default)]
pub(crate) struct RuntimeState {
    next_handle: i64,
    resources: HashMap<i64, Resource>,
    owned_strings: HashSet<usize>,
}

enum Resource {
    Bytes(Vec<u8>),
    TcpStream(TcpStream),
}

impl Resource {
    fn kind(&self) -> HandleKind {
        match self {
            Resource::Bytes(_) => HandleKind::Bytes,
            Resource::TcpStream(_) => HandleKind::TcpStream,
        }
    }
}

impl RuntimeState {
    pub(crate) fn insert_stream(&mut self, stream: TcpStream) -> i64 {
        let handle = self.next_handle();
        self.resources.insert(handle, Resource::TcpStream(stream));
        handle
    }

    pub(crate) fn stream(&self, handle: i64) -> Result<&TcpStream, String> {
        match self.resources.get(&handle) {
            Some(Resource::TcpStream(stream)) => Ok(stream),
            Some(resource) => Err(wrong_kind_error(handle, HandleKind::TcpStream, resource.kind())),
            None => Err(format!("tcp stream handle {handle} is closed or invalid")),
        }
    }

    pub(crate) fn insert_bytes(&mut self, bytes: Vec<u8>) -> i64 {
        let handle = self.next_handle();
        self.resources.insert(handle, Resource::Bytes(bytes));
        handle
    }

    pub(crate) fn take_bytes(&mut self, handle: i64) -> Result<Vec<u8>, String> {
        let Some(resource) = self.resources.remove(&handle) else {
            return Err(format!("bytes handle {handle} is closed or invalid"));
        };
        match resource {
            Resource::Bytes(bytes) => Ok(bytes),
            other => {
                let actual = other.kind();
                self.resources.insert(handle, other);
                Err(wrong_kind_error(handle, HandleKind::Bytes, actual))
            }
        }
    }

    pub(crate) fn close_any(&mut self, handle: i64) -> bool {
        self.resources.remove(&handle).is_some()
    }

    pub(crate) fn close_kind(&mut self, handle: i64, expected: HandleKind) -> Result<bool, String> {
        let Some(resource) = self.resources.get(&handle) else {
            return Ok(false);
        };
        let actual = resource.kind();
        if actual != expected {
            return Err(wrong_kind_error(handle, expected, actual));
        }
        self.resources.remove(&handle);
        Ok(true)
    }

    fn next_handle(&mut self) -> i64 {
        self.next_handle += 1;
        self.next_handle
    }

    pub(crate) fn register_string(&mut self, ptr: *mut c_char) {
        if !ptr.is_null() {
            self.owned_strings.insert(ptr as usize);
        }
    }

    pub(crate) fn unregister_string(&mut self, ptr: *mut c_char) -> bool {
        self.owned_strings.remove(&(ptr as usize))
    }

    pub(crate) fn cleanup(&mut self) {
        self.resources.clear();
        for ptr in self.owned_strings.drain() {
            // SAFETY: All entries are pointers produced by CString::into_raw
            // and registered by lkrt before being returned across FFI.
            unsafe {
                drop(CString::from_raw(ptr as *mut c_char));
            }
        }
    }
}

fn wrong_kind_error(handle: i64, expected: HandleKind, actual: HandleKind) -> String {
    format!("handle {handle} has kind {actual:?}, expected {expected:?}")
}
