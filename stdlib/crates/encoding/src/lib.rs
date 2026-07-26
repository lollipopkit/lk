#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

// From `alloc` directly, not `lk_core::compat::prelude`: feature
// unification can give lk-core `std` while this crate stays no_std, and
// then that prelude does not exist. What alloc provides does not depend
// on anyone else's features.
#[cfg(not(feature = "std"))]
#[allow(unused_imports)]
use alloc::{
    borrow::ToOwned,
    boxed::Box,
    format,
    string::{String, ToString},
    vec,
    vec::Vec,
};

#[cfg(feature = "std")]
use alloc::sync::Arc;
#[cfg(feature = "std")]
use anyhow::bail;
use anyhow::{Result, anyhow};
use base64::Engine as _;
#[cfg(feature = "std")]
use lk_core::util::fast_map::fast_hash_map_new;
#[cfg(feature = "std")]
use lk_core::val::{HeapValue, TypedMap};
use lk_core::{
    val::{RuntimeVal, de},
    vm::{NativeArgs, NativeRuntime},
};
use lk_stdlib_bytes::{runtime_bytes_or_string_arg, runtime_bytes_value};
use lk_stdlib_common::runtime_native::{parse_format, runtime_string_arg, runtime_string_value};

#[derive(Debug, Default, lk_stdlib_common::StdlibModule)]
#[stdlib_module(name = "encoding", docs = "Encoding and data format helpers")]
pub struct EncodingModule;

// The child list is a proc-macro argument, so `#[cfg]` cannot prune entries
// from inside it; both sets are spelled out instead.
#[cfg_attr(
    feature = "std",
    lk_stdlib_common::stdlib_exports(children(
        json = JsonModule,
        yaml = YamlModule,
        toml = TomlModule,
        base64 = Base64Module,
        hex = HexModule,
        url = UrlEncodingModule,
    ))
)]
#[cfg_attr(
    not(feature = "std"),
    lk_stdlib_common::stdlib_exports(children(json = JsonModule, base64 = Base64Module, hex = HexModule,))
)]
impl EncodingModule {}

#[derive(Debug, Default, lk_stdlib_common::StdlibModule)]
#[stdlib_module(name = "json", docs = "JSON parser")]
struct JsonModule;

#[lk_stdlib_common::stdlib_exports(module = "encoding.json")]
impl JsonModule {
    #[stdlib_export(params(source: String), returns = Value)]
    fn parse(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        parse_format(args, runtime, "encoding.json.parse", de::Format::Json)
    }
}

#[cfg(feature = "std")]
#[derive(Debug, Default, lk_stdlib_common::StdlibModule)]
#[stdlib_module(name = "yaml", docs = "YAML parser")]
struct YamlModule;

#[cfg(feature = "std")]
#[lk_stdlib_common::stdlib_exports(module = "encoding.yaml")]
impl YamlModule {
    #[stdlib_export(params(source: String), returns = Value)]
    fn parse(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        parse_format(args, runtime, "encoding.yaml.parse", de::Format::Yaml)
    }
}

#[cfg(feature = "std")]
#[derive(Debug, Default, lk_stdlib_common::StdlibModule)]
#[stdlib_module(name = "toml", docs = "TOML parser")]
struct TomlModule;

#[cfg(feature = "std")]
#[lk_stdlib_common::stdlib_exports(module = "encoding.toml")]
impl TomlModule {
    #[stdlib_export(params(source: String), returns = Value)]
    fn parse(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        parse_format(args, runtime, "encoding.toml.parse", de::Format::Toml)
    }
}

#[derive(Debug, Default, lk_stdlib_common::StdlibModule)]
#[stdlib_module(name = "base64", docs = "Base64 encoding helpers")]
struct Base64Module;

#[lk_stdlib_common::stdlib_exports(module = "encoding.base64")]
impl Base64Module {
    #[stdlib_export(params(data: Bytes | String), returns = String)]
    fn encode(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let data = runtime_bytes_or_string_arg(
            args.get(0).expect("checked arity"),
            runtime.heap(),
            "encoding.base64.encode data",
        )?;
        Ok(runtime_string_value(
            &base64::engine::general_purpose::STANDARD.encode(data.as_ref()),
            runtime.heap_mut(),
        ))
    }

    #[stdlib_export(params(data: String), returns = Bytes)]
    fn decode(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let data = runtime_string_arg(
            args.get(0).expect("checked arity"),
            runtime.heap(),
            "encoding.base64.decode data",
        )?;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(data.as_bytes())
            .map_err(|err| anyhow!("invalid base64 data: {err}"))?;
        Ok(runtime_bytes_value(bytes, runtime.heap_mut()))
    }
}

#[derive(Debug, Default, lk_stdlib_common::StdlibModule)]
#[stdlib_module(name = "hex", docs = "Hex encoding helpers")]
struct HexModule;

#[lk_stdlib_common::stdlib_exports(module = "encoding.hex")]
impl HexModule {
    #[stdlib_export(params(data: Bytes | String), returns = String)]
    fn encode(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let data = runtime_bytes_or_string_arg(
            args.get(0).expect("checked arity"),
            runtime.heap(),
            "encoding.hex.encode data",
        )?;
        Ok(runtime_string_value(&hex::encode(data.as_ref()), runtime.heap_mut()))
    }

    #[stdlib_export(params(data: String), returns = Bytes)]
    fn decode(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let data = runtime_string_arg(
            args.get(0).expect("checked arity"),
            runtime.heap(),
            "encoding.hex.decode data",
        )?;
        let bytes = hex::decode(data.as_ref()).map_err(|err| anyhow!("invalid hex data: {err}"))?;
        Ok(runtime_bytes_value(bytes, runtime.heap_mut()))
    }
}

#[cfg(feature = "std")]
#[derive(Debug, Default, lk_stdlib_common::StdlibModule)]
#[stdlib_module(name = "url", docs = "URL encoding helpers")]
struct UrlEncodingModule;

#[cfg(feature = "std")]
#[lk_stdlib_common::stdlib_exports(module = "encoding.url")]
impl UrlEncodingModule {
    #[stdlib_export(params(value: String), returns = String)]
    fn encode_component(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let value = runtime_string_arg(
            args.get(0).expect("checked arity"),
            runtime.heap(),
            "encoding.url.encode_component value",
        )?;
        Ok(runtime_string_value(
            &url::form_urlencoded::byte_serialize(value.as_bytes()).collect::<String>(),
            runtime.heap_mut(),
        ))
    }

    #[stdlib_export(params(value: String), returns = String)]
    fn decode_component(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let value = runtime_string_arg(
            args.get(0).expect("checked arity"),
            runtime.heap(),
            "encoding.url.decode_component value",
        )?;
        let decoded = percent_decode_component(value.as_ref())?;
        Ok(runtime_string_value(&decoded, runtime.heap_mut()))
    }

    #[stdlib_export(params(query: String), returns = Map)]
    fn query_parse(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let value = runtime_string_arg(
            args.get(0).expect("checked arity"),
            runtime.heap(),
            "encoding.url.query_parse value",
        )?;
        let mut map = fast_hash_map_new();
        for (key, value) in url::form_urlencoded::parse(value.as_bytes()) {
            map.insert(
                Arc::<str>::from(key.as_ref()),
                runtime_string_value(value.as_ref(), runtime.heap_mut()),
            );
        }
        Ok(RuntimeVal::Obj(
            runtime.heap_mut().alloc(HeapValue::Map(TypedMap::StringMixed(map))),
        ))
    }

    #[stdlib_export(params(map: Map<String, String>), returns = String)]
    fn query_stringify(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let map = string_map_arg(
            args.get(0).expect("checked arity"),
            runtime,
            "encoding.url.query_stringify map",
        )?;
        let mut serializer = url::form_urlencoded::Serializer::new(String::new());
        for (key, value) in map {
            serializer.append_pair(&key, &value);
        }
        Ok(runtime_string_value(&serializer.finish(), runtime.heap_mut()))
    }
}

/// Only used by the `url` child, which is std-only.
#[cfg(feature = "std")]
fn percent_decode_component(value: &str) -> Result<String> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let Some(hex) = bytes.get(index + 1..index + 3) else {
                bail!("invalid percent encoding: incomplete escape");
            };
            let hex = std::str::from_utf8(hex).map_err(|_| anyhow!("invalid percent encoding: non-UTF-8 escape"))?;
            let byte = u8::from_str_radix(hex, 16)
                .map_err(|_| anyhow!("invalid percent encoding: expected two hex digits"))?;
            decoded.push(byte);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(decoded).map_err(|err| anyhow!("invalid percent-encoded UTF-8: {err}"))
}

/// Only used by the `url` child, which is std-only.
#[cfg(feature = "std")]
fn string_map_arg(value: &RuntimeVal, runtime: &NativeRuntime<'_>, context: &str) -> Result<Vec<(String, String)>> {
    let RuntimeVal::Obj(handle) = value else {
        bail!("{context} expects map");
    };
    let value = runtime
        .heap()
        .get(*handle)
        .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?;
    let HeapValue::Map(map) = value else {
        bail!("{context} expects map, got {}", value.type_name());
    };
    match map {
        TypedMap::StringMixed(values) => values
            .iter()
            .map(|(key, value)| {
                Ok((
                    key.to_string(),
                    runtime_string_arg(value, runtime.heap(), context)?.to_string(),
                ))
            })
            .collect(),
        _ => bail!("{context} expects string map"),
    }
}
