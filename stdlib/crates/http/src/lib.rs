use anyhow::{Result, anyhow, bail};
use lk_core::{
    module::{ModuleProvider, ModuleRegistry},
    util::fast_map::fast_hash_map_new,
    val::{HeapValue, RuntimeVal, TypedMap},
    vm::{NativeArgs, NativeRuntime, RuntimeExport},
};
use lk_stdlib_bytes::{runtime_bytes_or_string_arg, runtime_bytes_value};
use lk_stdlib_common::runtime_native::{runtime_string_arg, runtime_string_value};
use std::{io::Read, sync::Arc, time::Duration};

const MAX_BODY_BYTES: usize = 16 * 1024 * 1024;

#[derive(Debug, Default)]
pub struct HttpModule;

impl HttpModule {
    pub fn new() -> Self {
        Self
    }
}

impl ModuleProvider for HttpModule {
    fn name(&self) -> &str {
        "http"
    }

    fn register(&self, _registry: &mut ModuleRegistry) -> Result<()> {
        Ok(())
    }

    fn runtime_exports(&self) -> Result<RuntimeExport> {
        Ok(lk_stdlib_common::stdlib_runtime_exports!(
            [
                plain "request" => request, lk_core::vm::NativeEntry::VARIADIC,
                plain "get" => get, lk_core::vm::NativeEntry::VARIADIC,
                plain "post" => post, lk_core::vm::NativeEntry::VARIADIC,
            ],
        ))
    }
}

pub fn register(registry: &mut ModuleRegistry) -> Result<()> {
    registry.register_module("http", Box::new(HttpModule::new()))
}

fn request(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    if args.len() < 2 || args.len() > 3 {
        bail!("http.request() expects 2 or 3 arguments: method, url[, opts]");
    }
    let method = runtime_string_arg(
        args.get(0).expect("checked arity"),
        runtime.heap(),
        "http.request method",
    )?;
    let url = runtime_string_arg(args.get(1).expect("checked arity"), runtime.heap(), "http.request url")?;
    let opts = args.get(2);
    send_request(method.as_ref(), url.as_ref(), opts, None, runtime)
}

fn get(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    if args.is_empty() || args.len() > 2 {
        bail!("http.get() expects 1 or 2 arguments: url[, opts]");
    }
    let url = runtime_string_arg(args.get(0).expect("checked arity"), runtime.heap(), "http.get url")?;
    send_request("GET", url.as_ref(), args.get(1), None, runtime)
}

fn post(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    if args.len() < 2 || args.len() > 3 {
        bail!("http.post() expects 2 or 3 arguments: url, body[, opts]");
    }
    let url = runtime_string_arg(args.get(0).expect("checked arity"), runtime.heap(), "http.post url")?;
    let body = runtime_bytes_or_string_arg(args.get(1).expect("checked arity"), runtime.heap(), "http.post body")?;
    send_request("POST", url.as_ref(), args.get(2), Some(body), runtime)
}

fn send_request(
    method: &str,
    url: &str,
    opts: Option<&RuntimeVal>,
    body: Option<Arc<[u8]>>,
    runtime: &mut NativeRuntime<'_>,
) -> Result<RuntimeVal> {
    let config = HttpOptions::from_runtime(opts, runtime)?;
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_millis(config.timeout_ms))
        .build();
    let mut request = agent.request(method, url);
    for (key, value) in config.headers {
        request = request.set(&key, &value);
    }
    let response = match match body {
        Some(body) => request.send_bytes(&body),
        None => request.call(),
    } {
        Ok(response) => response,
        Err(ureq::Error::Status(_, response)) => response,
        Err(err) => return Err(anyhow!("http request failed: {err}")),
    };
    response_map(response, runtime)
}

fn response_map(response: ureq::Response, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    let status = response.status() as i64;
    let mut headers = fast_hash_map_new();
    for name in response.headers_names() {
        if let Some(value) = response.header(&name) {
            headers.insert(Arc::<str>::from(name), runtime_string_value(value, runtime.heap_mut()));
        }
    }
    let mut reader = response.into_reader().take((MAX_BODY_BYTES + 1) as u64);
    let mut body = Vec::new();
    reader
        .read_to_end(&mut body)
        .map_err(|err| anyhow!("failed to read response body: {err}"))?;
    if body.len() > MAX_BODY_BYTES {
        bail!("http response body exceeds {MAX_BODY_BYTES} bytes");
    }
    let headers = RuntimeVal::Obj(runtime.heap_mut().alloc(HeapValue::Map(TypedMap::StringMixed(headers))));
    let mut map = fast_hash_map_new();
    map.insert(Arc::<str>::from("status"), RuntimeVal::Int(status));
    map.insert(Arc::<str>::from("headers"), headers);
    map.insert(Arc::<str>::from("body"), runtime_bytes_value(body, runtime.heap_mut()));
    Ok(RuntimeVal::Obj(
        runtime.heap_mut().alloc(HeapValue::Map(TypedMap::StringMixed(map))),
    ))
}

#[derive(Default)]
struct HttpOptions {
    timeout_ms: u64,
    headers: Vec<(String, String)>,
}

impl HttpOptions {
    fn from_runtime(value: Option<&RuntimeVal>, runtime: &NativeRuntime<'_>) -> Result<Self> {
        let mut out = Self {
            timeout_ms: 30_000,
            headers: Vec::new(),
        };
        let Some(value) = value else {
            return Ok(out);
        };
        let RuntimeVal::Obj(handle) = value else {
            bail!("http opts must be a map");
        };
        let value = runtime
            .heap()
            .get(*handle)
            .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?;
        let HeapValue::Map(TypedMap::StringMixed(map)) = value else {
            bail!("http opts must be a string map");
        };
        if let Some(value) = map.get("timeout_ms") {
            out.timeout_ms = int_arg(value, "http opts.timeout_ms")? as u64;
        }
        if let Some(value) = map.get("headers") {
            out.headers = headers_arg(value, runtime, "http opts.headers")?;
        }
        Ok(out)
    }
}

fn headers_arg(value: &RuntimeVal, runtime: &NativeRuntime<'_>, context: &str) -> Result<Vec<(String, String)>> {
    let RuntimeVal::Obj(handle) = value else {
        bail!("{context} expects map");
    };
    let value = runtime
        .heap()
        .get(*handle)
        .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?;
    let HeapValue::Map(TypedMap::StringMixed(map)) = value else {
        bail!("{context} expects string map");
    };
    map.iter()
        .map(|(key, value)| {
            Ok((
                key.to_string(),
                runtime_string_arg(value, runtime.heap(), context)?.to_string(),
            ))
        })
        .collect()
}

fn int_arg(value: &RuntimeVal, context: &str) -> Result<i64> {
    match value {
        RuntimeVal::Int(value) if *value >= 0 => Ok(*value),
        other => bail!("{context} expects non-negative Int, got {:?}", other.kind()),
    }
}
