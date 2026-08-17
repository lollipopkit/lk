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
use lk_core::util::value_map::value_map_new;
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

    /// The other half of `parse`. Without it a script could read a config and
    /// change it but not write it back — two thirds of the most ordinary task
    /// there is.
    #[stdlib_export(params(value: Value), returns = String)]
    fn stringify(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        write_format(
            args,
            runtime,
            "encoding.json.stringify",
            lk_core::val::ser::to_json_string,
        )
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

    #[stdlib_export(params(value: Value), returns = String)]
    fn stringify(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        write_format(
            args,
            runtime,
            "encoding.yaml.stringify",
            lk_core::val::ser::to_yaml_string,
        )
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

    #[stdlib_export(params(value: Value), returns = String)]
    fn stringify(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        write_format(
            args,
            runtime,
            "encoding.toml.stringify",
            lk_core::val::ser::to_toml_string,
        )
    }
}

/// The `stringify` half of `parse_format`: one argument in, text out.
fn write_format(
    args: NativeArgs<'_>,
    runtime: &mut NativeRuntime<'_>,
    name: &str,
    write: fn(&RuntimeVal, &lk_core::val::HeapStore) -> Result<String>,
) -> Result<RuntimeVal> {
    if args.len() != 1 {
        return Err(anyhow!("{name}(value) requires 1 argument"));
    }
    let text =
        write(args.get(0).expect("checked arity"), runtime.heap()).map_err(|error| anyhow!("{name}: {error}"))?;
    Ok(runtime_string_value(&text, runtime.heap_mut()))
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
            &percent_encode_component(value.as_ref()),
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
        let mut map = value_map_new();
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

/// Percent-encodes a URI **component**: everything outside the unreserved set
/// becomes `%XX`.
///
/// The other direction of [`percent_decode_component`], written here rather than
/// taken from a crate so the pair is one implementation's two directions. It used
/// to be `form_urlencoded::byte_serialize`, which is *form* encoding — a space
/// becomes `+` — while the decoder only ever undid `%XX`. So the pair did not
/// round-trip: `decode_component(encode_component("a b"))` was `"a+b"`.
///
/// Form encoding is what a query body wants, and `query_stringify` /
/// `query_parse` are that pair; they use `form_urlencoded` on both sides and are
/// unaffected. A *component* keeps `+` as the literal `+` it is, which is also
/// what `encodeURIComponent` / `decodeURIComponent` do.
///
/// The unreserved set is `encodeURIComponent`'s: `A-Za-z0-9-_.!~*'()`.
#[cfg(feature = "std")]
fn percent_encode_component(value: &str) -> String {
    fn unreserved(byte: u8) -> bool {
        byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'!' | b'~' | b'*' | b'\'' | b'(' | b')')
    }
    let mut out = String::with_capacity(value.len());
    for &byte in value.as_bytes() {
        if unreserved(byte) {
            out.push(byte as char);
        } else {
            out.push('%');
            out.push_str(&alloc::format!("{byte:02X}"));
        }
    }
    out
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

#[cfg(all(test, feature = "std"))]
mod component_tests {
    use super::{percent_decode_component, percent_encode_component};

    /// The pair's two directions have to agree with each other before they agree
    /// with anything else. They did not: the encoder was `form_urlencoded`'s
    /// *form* encoding (a space becomes `+`) while the decoder only ever undid
    /// `%XX`, so `decode(encode("a b"))` was `"a+b"`.
    #[test]
    fn a_component_round_trips() {
        for original in [
            "a b&c=d",
            "",
            "plain",
            "+literal+",
            "100%",
            "héllo",
            "a/b?c#d",
            "~*'()!-_.",
        ] {
            let encoded = percent_encode_component(original);
            let decoded = percent_decode_component(&encoded).expect("own output decodes");
            assert_eq!(decoded, original, "round trip of {original:?} through {encoded:?}");
        }
    }

    /// A space is `%20`, and `+` is the literal `+` — `encodeURIComponent`'s
    /// rule. Form encoding is what a query body wants, and `query_stringify` /
    /// `query_parse` are that pair, on `form_urlencoded` at both ends.
    #[test]
    fn a_component_is_not_form_encoded() {
        assert_eq!(percent_encode_component("a b"), "a%20b");
        assert_eq!(percent_decode_component("a+b").expect("valid"), "a+b");
        // The unreserved set survives untouched.
        assert_eq!(percent_encode_component("aZ09-_.!~*'()"), "aZ09-_.!~*'()");
    }
}
