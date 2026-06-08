use anyhow::{Result, anyhow, bail};
use base64::Engine as _;
use lk_core::{
    module::{ModuleProvider, ModuleRegistry, RuntimeNativeExport, runtime_export_from_plain_native_entries},
    util::fast_map::fast_hash_map_new,
    val::{HeapValue, RuntimeVal, TypedMap, de},
    vm::{Module, NativeArgs, NativeRuntime, RuntimeExport, RuntimeModuleState, import_runtime_export},
};
use lk_stdlib_bytes::{runtime_bytes_or_string_arg, runtime_bytes_value};
use lk_stdlib_common::runtime_native::{parse_format, runtime_string_arg, runtime_string_value};
use std::sync::{Arc, Mutex};

#[derive(Debug, Default)]
pub struct EncodingModule;

impl EncodingModule {
    pub fn new() -> Self {
        Self
    }
}

impl ModuleProvider for EncodingModule {
    fn name(&self) -> &str {
        "encoding"
    }

    fn register(&self, _registry: &mut ModuleRegistry) -> Result<()> {
        Ok(())
    }

    fn runtime_exports(&self) -> Result<RuntimeExport> {
        namespace_export(&[
            (
                "json",
                DataFormatModule::new("json", de::Format::Json).runtime_exports()?,
            ),
            (
                "yaml",
                DataFormatModule::new("yaml", de::Format::Yaml).runtime_exports()?,
            ),
            (
                "toml",
                DataFormatModule::new("toml", de::Format::Toml).runtime_exports()?,
            ),
            ("base64", Base64Module.runtime_exports()?),
            ("hex", HexModule.runtime_exports()?),
            ("url", UrlEncodingModule.runtime_exports()?),
        ])
    }
}

pub fn register(registry: &mut ModuleRegistry) -> Result<()> {
    registry.register_module("encoding", Box::new(EncodingModule::new()))
}

#[derive(Debug)]
struct DataFormatModule {
    name: &'static str,
    format: de::Format,
}

impl DataFormatModule {
    fn new(name: &'static str, format: de::Format) -> Self {
        Self { name, format }
    }
}

impl ModuleProvider for DataFormatModule {
    fn name(&self) -> &str {
        self.name
    }

    fn register(&self, _registry: &mut ModuleRegistry) -> Result<()> {
        Ok(())
    }

    fn runtime_exports(&self) -> Result<RuntimeExport> {
        let parse_fn = match self.format {
            de::Format::Json => parse_json,
            de::Format::Yaml => parse_yaml,
            de::Format::Toml => parse_toml,
        };
        Ok(runtime_export_from_plain_native_entries(
            &[RuntimeNativeExport::plain("parse", parse_fn, 1)],
            &[],
        ))
    }
}

#[derive(Debug)]
struct Base64Module;

impl ModuleProvider for Base64Module {
    fn name(&self) -> &str {
        "base64"
    }

    fn register(&self, _registry: &mut ModuleRegistry) -> Result<()> {
        Ok(())
    }

    fn runtime_exports(&self) -> Result<RuntimeExport> {
        Ok(runtime_export_from_plain_native_entries(
            &[
                RuntimeNativeExport::plain("encode", base64_encode, 1),
                RuntimeNativeExport::plain("decode", base64_decode, 1),
            ],
            &[],
        ))
    }
}

#[derive(Debug)]
struct HexModule;

impl ModuleProvider for HexModule {
    fn name(&self) -> &str {
        "hex"
    }

    fn register(&self, _registry: &mut ModuleRegistry) -> Result<()> {
        Ok(())
    }

    fn runtime_exports(&self) -> Result<RuntimeExport> {
        Ok(runtime_export_from_plain_native_entries(
            &[
                RuntimeNativeExport::plain("encode", hex_encode, 1),
                RuntimeNativeExport::plain("decode", hex_decode, 1),
            ],
            &[],
        ))
    }
}

#[derive(Debug)]
struct UrlEncodingModule;

impl ModuleProvider for UrlEncodingModule {
    fn name(&self) -> &str {
        "url"
    }

    fn register(&self, _registry: &mut ModuleRegistry) -> Result<()> {
        Ok(())
    }

    fn runtime_exports(&self) -> Result<RuntimeExport> {
        Ok(runtime_export_from_plain_native_entries(
            &[
                RuntimeNativeExport::plain("encode_component", url_encode_component, 1),
                RuntimeNativeExport::plain("decode_component", url_decode_component, 1),
                RuntimeNativeExport::plain("query_parse", query_parse, 1),
                RuntimeNativeExport::plain("query_stringify", query_stringify, 1),
            ],
            &[],
        ))
    }
}

fn parse_json(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    parse_format(args, runtime, "encoding.json.parse", de::Format::Json)
}

fn parse_yaml(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    parse_format(args, runtime, "encoding.yaml.parse", de::Format::Yaml)
}

fn parse_toml(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    parse_format(args, runtime, "encoding.toml.parse", de::Format::Toml)
}

fn base64_encode(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 1, "encoding.base64.encode()")?;
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

fn base64_decode(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 1, "encoding.base64.decode()")?;
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

fn hex_encode(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 1, "encoding.hex.encode()")?;
    let data = runtime_bytes_or_string_arg(
        args.get(0).expect("checked arity"),
        runtime.heap(),
        "encoding.hex.encode data",
    )?;
    Ok(runtime_string_value(&hex::encode(data.as_ref()), runtime.heap_mut()))
}

fn hex_decode(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 1, "encoding.hex.decode()")?;
    let data = runtime_string_arg(
        args.get(0).expect("checked arity"),
        runtime.heap(),
        "encoding.hex.decode data",
    )?;
    let bytes = hex::decode(data.as_ref()).map_err(|err| anyhow!("invalid hex data: {err}"))?;
    Ok(runtime_bytes_value(bytes, runtime.heap_mut()))
}

fn url_encode_component(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 1, "encoding.url.encode_component()")?;
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

fn url_decode_component(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 1, "encoding.url.decode_component()")?;
    let value = runtime_string_arg(
        args.get(0).expect("checked arity"),
        runtime.heap(),
        "encoding.url.decode_component value",
    )?;
    let wrapped = format!("x={value}");
    let decoded = url::form_urlencoded::parse(wrapped.as_bytes())
        .map(|(_, value)| value.to_string())
        .next()
        .unwrap_or_default();
    Ok(runtime_string_value(&decoded, runtime.heap_mut()))
}

fn query_parse(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 1, "encoding.url.query_parse()")?;
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

fn query_stringify(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 1, "encoding.url.query_stringify()")?;
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

fn namespace_export(entries: &[(&'static str, RuntimeExport)]) -> Result<RuntimeExport> {
    let mut heap = lk_core::val::HeapStore::new();
    let mut map = fast_hash_map_new();
    for (name, export) in entries {
        map.insert(Arc::<str>::from(*name), import_runtime_export(export, &mut heap)?);
    }
    let value = RuntimeVal::Obj(heap.alloc(HeapValue::Map(TypedMap::StringMixed(map))));
    Ok(RuntimeExport::new(
        value,
        Arc::new(Mutex::new(RuntimeModuleState::new(heap, Vec::new()))),
        Arc::new(Module::default()),
    ))
}

fn expect_arity(args: NativeArgs<'_>, expected: usize, name: &str) -> Result<()> {
    if args.len() == expected {
        Ok(())
    } else {
        bail!("{name} expects exactly {expected} argument(s)")
    }
}
