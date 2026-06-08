use anyhow::{Result, anyhow, bail};
use lk_core::{
    module::{ModuleProvider, ModuleRegistry, RuntimeNativeExport, runtime_export_from_plain_native_entries},
    util::fast_map::fast_hash_map_new,
    val::{HeapValue, RuntimeVal, TypedList, TypedMap},
    vm::{NativeArgs, NativeRuntime, RuntimeExport},
};
use lk_stdlib_common::runtime_native::{runtime_string_arg, runtime_string_value};
use once_cell::sync::Lazy;
use std::sync::{Arc, Mutex};

static CACHE: Lazy<Mutex<std::collections::HashMap<String, regex::Regex>>> =
    Lazy::new(|| Mutex::new(std::collections::HashMap::new()));

#[derive(Debug, Default)]
pub struct RegexModule;

impl RegexModule {
    pub fn new() -> Self {
        Self
    }
}

impl ModuleProvider for RegexModule {
    fn name(&self) -> &str {
        "regex"
    }

    fn register(&self, _registry: &mut ModuleRegistry) -> Result<()> {
        Ok(())
    }

    fn runtime_exports(&self) -> Result<RuntimeExport> {
        Ok(runtime_export_from_plain_native_entries(
            &[
                RuntimeNativeExport::plain("is_match", is_match, 2),
                RuntimeNativeExport::plain("find", find, 2),
                RuntimeNativeExport::plain("find_all", find_all, 2),
                RuntimeNativeExport::plain("captures", captures, 2),
                RuntimeNativeExport::plain("replace", replace, 3),
                RuntimeNativeExport::plain("split", split, 2),
            ],
            &[],
        ))
    }
}

pub fn register(registry: &mut ModuleRegistry) -> Result<()> {
    registry.register_module("regex", Box::new(RegexModule::new()))
}

fn is_match(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    let (regex, text) = regex_text(args, runtime, "regex.is_match()")?;
    Ok(RuntimeVal::Bool(regex.is_match(text.as_ref())))
}

fn find(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    let (regex, text) = regex_text(args, runtime, "regex.find()")?;
    Ok(match regex.find(text.as_ref()) {
        Some(m) => match_map(m.as_str(), m.start(), m.end(), runtime),
        None => RuntimeVal::Nil,
    })
}

fn find_all(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    let (regex, text) = regex_text(args, runtime, "regex.find_all()")?;
    let values = regex
        .find_iter(text.as_ref())
        .map(|m| match_map(m.as_str(), m.start(), m.end(), runtime))
        .collect::<Vec<_>>();
    let list = lk_stdlib_common::typed_list_from_values(values, runtime.heap());
    Ok(RuntimeVal::Obj(runtime.heap_mut().alloc(HeapValue::List(list))))
}

fn captures(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    let (regex, text) = regex_text(args, runtime, "regex.captures()")?;
    let Some(captures) = regex.captures(text.as_ref()) else {
        return Ok(RuntimeVal::Nil);
    };
    let mut values = Vec::new();
    for capture in captures.iter() {
        values.push(match capture {
            Some(value) => runtime_string_value(value.as_str(), runtime.heap_mut()),
            None => RuntimeVal::Nil,
        });
    }
    let list = lk_stdlib_common::typed_list_from_values(values, runtime.heap());
    Ok(RuntimeVal::Obj(runtime.heap_mut().alloc(HeapValue::List(list))))
}

fn replace(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 3, "regex.replace()")?;
    let regex = cached_regex(args.get(0).expect("checked arity"), runtime, "regex.replace pattern")?;
    let text = runtime_string_arg(
        args.get(1).expect("checked arity"),
        runtime.heap(),
        "regex.replace text",
    )?;
    let replacement = runtime_string_arg(
        args.get(2).expect("checked arity"),
        runtime.heap(),
        "regex.replace replacement",
    )?;
    Ok(runtime_string_value(
        &regex.replace_all(text.as_ref(), replacement.as_ref()),
        runtime.heap_mut(),
    ))
}

fn split(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    let (regex, text) = regex_text(args, runtime, "regex.split()")?;
    let values = regex.split(text.as_ref()).map(Arc::<str>::from).collect::<Vec<_>>();
    Ok(RuntimeVal::Obj(
        runtime.heap_mut().alloc(HeapValue::List(TypedList::String(values))),
    ))
}

fn regex_text(args: NativeArgs<'_>, runtime: &NativeRuntime<'_>, name: &str) -> Result<(regex::Regex, Arc<str>)> {
    expect_arity(args, 2, name)?;
    let regex = cached_regex(args.get(0).expect("checked arity"), runtime, name)?;
    let text = runtime_string_arg(args.get(1).expect("checked arity"), runtime.heap(), name)?;
    Ok((regex, text))
}

fn cached_regex(value: &RuntimeVal, runtime: &NativeRuntime<'_>, context: &str) -> Result<regex::Regex> {
    let pattern = runtime_string_arg(value, runtime.heap(), context)?;
    {
        let cache = CACHE.lock().map_err(|_| anyhow!("regex cache lock poisoned"))?;
        if let Some(regex) = cache.get(pattern.as_ref()) {
            return Ok(regex.clone());
        }
    }
    let regex = regex::Regex::new(pattern.as_ref()).map_err(|err| anyhow!("invalid regex: {err}"))?;
    let mut cache = CACHE.lock().map_err(|_| anyhow!("regex cache lock poisoned"))?;
    if let Some(regex) = cache.get(pattern.as_ref()) {
        return Ok(regex.clone());
    }
    if cache.len() >= 128 {
        cache.clear();
    }
    cache.insert(pattern.to_string(), regex.clone());
    Ok(regex)
}

fn match_map(text: &str, start: usize, end: usize, runtime: &mut NativeRuntime<'_>) -> RuntimeVal {
    let mut map = fast_hash_map_new();
    map.insert(Arc::<str>::from("text"), runtime_string_value(text, runtime.heap_mut()));
    map.insert(Arc::<str>::from("start"), RuntimeVal::Int(start as i64));
    map.insert(Arc::<str>::from("end"), RuntimeVal::Int(end as i64));
    RuntimeVal::Obj(runtime.heap_mut().alloc(HeapValue::Map(TypedMap::StringMixed(map))))
}

fn expect_arity(args: NativeArgs<'_>, expected: usize, name: &str) -> Result<()> {
    if args.len() == expected {
        Ok(())
    } else {
        bail!("{name} expects exactly {expected} argument(s)")
    }
}
