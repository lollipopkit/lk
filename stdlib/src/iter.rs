use anyhow::{Result, anyhow};
use lkr_core::module::Module;
use lkr_core::val::methods::register_method;
use lkr_core::val::{IteratorState, IteratorValue, Val};
use lkr_core::vm::VmContext;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};

static LEGACY_WARNINGS: OnceLock<Mutex<HashSet<&'static str>>> = OnceLock::new();

fn warn_legacy(api: &'static str, message: &'static str) {
    let inserted = {
        let registry = LEGACY_WARNINGS.get_or_init(|| Mutex::new(HashSet::new()));
        let mut guard = registry.lock().expect("legacy warning mutex poisoned");
        guard.insert(api)
    };
    if inserted {
        tracing::warn!(target: "lkr::iter", "{}: {}", api, message);
    }
}

fn expect_iterator(val: &Val, func: &str) -> Result<Arc<IteratorValue>> {
    match val {
        Val::Iterator(handle) => Ok(handle.clone()),
        other => Err(anyhow!("{} expects iterator, got {}", func, other.type_name())),
    }
}

fn expect_callable(val: &Val, func: &str) -> Result<Val> {
    match val {
        f @ Val::Closure(_) | f @ Val::RustFunction(_) | f @ Val::RustFunctionNamed(_) => Ok(f.clone()),
        other => Err(anyhow!("{} expects function, got {}", func, other.type_name())),
    }
}

fn collect_iterator_to_list(iter: Arc<IteratorValue>, ctx: &mut VmContext) -> Result<Val> {
    let (lower, upper) = iter.size_hint();
    let capacity = upper.unwrap_or(lower);
    let mut out: Vec<Val> = Vec::with_capacity(capacity);
    while let Some(value) = iter.next(ctx)? {
        out.push(value);
    }
    Ok(Val::List(out.into()))
}

fn truthy(value: &Val) -> bool {
    !matches!(value, Val::Bool(false) | Val::Nil)
}

struct MapIteratorState {
    source: Arc<IteratorValue>,
    mapper: Val,
}

impl IteratorState for MapIteratorState {
    fn next(&mut self, ctx: &mut VmContext) -> Result<Option<Val>> {
        match self.source.next(ctx)? {
            Some(value) => {
                let args = [value.clone()];
                let mapped = self.mapper.call(&args, ctx)?;
                Ok(Some(mapped))
            }
            None => Ok(None),
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.source.size_hint()
    }

    fn debug_name(&self) -> &'static str {
        "iterator.map"
    }
}

struct FilterIteratorState {
    source: Arc<IteratorValue>,
    predicate: Val,
}

impl IteratorState for FilterIteratorState {
    fn next(&mut self, ctx: &mut VmContext) -> Result<Option<Val>> {
        loop {
            match self.source.next(ctx)? {
                Some(candidate) => {
                    let args = [candidate.clone()];
                    let keep = self.predicate.call(&args, ctx)?;
                    if truthy(&keep) {
                        return Ok(Some(candidate));
                    }
                }
                None => return Ok(None),
            }
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let (_, upper) = self.source.size_hint();
        (0, upper)
    }

    fn debug_name(&self) -> &'static str {
        "iterator.filter"
    }
}

fn map_iterator_handle(source: Arc<IteratorValue>, mapper: Val) -> Result<Arc<IteratorValue>> {
    Ok(IteratorValue::with_origin(
        MapIteratorState { source, mapper },
        Arc::<str>::from("iter.map"),
    ))
}

fn filter_iterator_handle(source: Arc<IteratorValue>, predicate: Val) -> Result<Arc<IteratorValue>> {
    Ok(IteratorValue::with_origin(
        FilterIteratorState { source, predicate },
        Arc::<str>::from("iter.filter"),
    ))
}

#[derive(Debug)]
pub struct IterModule {
    functions: HashMap<String, Val>,
}

impl Default for IterModule {
    fn default() -> Self {
        Self::new()
    }
}

impl IterModule {
    pub fn new() -> Self {
        let mut functions = HashMap::new();

        // Register iterator functions as Rust functions
        // Generic higher-order ops
        functions.insert("map".to_string(), Val::RustFunction(map));
        functions.insert("filter".to_string(), Val::RustFunction(filter));
        functions.insert("reduce".to_string(), Val::RustFunction(reduce));
        // Sequence utilities
        functions.insert("enumerate".to_string(), Val::RustFunction(enumerate));
        functions.insert("range".to_string(), Val::RustFunction(range));
        functions.insert("zip".to_string(), Val::RustFunction(zip));
        functions.insert("take".to_string(), Val::RustFunction(take));
        functions.insert("skip".to_string(), Val::RustFunction(skip));
        functions.insert("chain".to_string(), Val::RustFunction(chain));
        functions.insert("flatten".to_string(), Val::RustFunction(flatten));
        functions.insert("unique".to_string(), Val::RustFunction(unique));
        functions.insert("chunk".to_string(), Val::RustFunction(chunk));
        functions.insert("next".to_string(), Val::RustFunction(next));
        functions.insert("collect".to_string(), Val::RustFunction(collect));

        register_method("Iterator", "map", iterator_map_method);
        register_method("Iterator", "filter", iterator_filter_method);
        register_method("Iterator", "reduce", iterator_reduce_method);
        register_method("Iterator", "next", iterator_next_method);
        register_method("Iterator", "collect", iterator_collect_method);

        Self { functions }
    }
}

impl Module for IterModule {
    fn name(&self) -> &'static str {
        "iter"
    }

    fn description(&self) -> &'static str {
        "Iterator utilities and functions for working with collections"
    }

    fn register(&self, _registry: &mut lkr_core::module::ModuleRegistry) -> Result<()> {
        // Don't register functions globally - they should be accessed via module.function()
        Ok(())
    }

    fn exports(&self) -> HashMap<String, Val> {
        self.functions.clone()
    }
}

/// map - apply function to each element of list or iterator
/// Accepts (receiver, function), including method sugar.
pub fn map(args: &[Val], ctx: &mut VmContext) -> Result<Val> {
    if let [Val::Iterator(iter), func] = args {
        let mapper = expect_callable(func, "map() second argument")?;
        let handle = map_iterator_handle(iter.clone(), mapper)?;
        warn_legacy(
            "iter.map(iterator)",
            "Returning materialised list for compatibility; prefer iterator.map(...).collect()",
        );
        return collect_iterator_to_list(handle, ctx);
    }
    if matches!(args, [Val::List(_), _]) {
        warn_legacy(
            "iter.map(list)",
            "Legacy eager map is deprecated; use list.into_iter().map(...).collect()",
        );
    }
    legacy_map(args, ctx)
}

fn legacy_map(args: &[Val], ctx: &mut VmContext) -> Result<Val> {
    let (list, func) = match args {
        [Val::List(l), f] => (l.clone(), f.clone()),
        _ => return Err(anyhow!("map() expects (list, function)")),
    };

    let call = match func {
        Val::Closure(_) | Val::RustFunction(_) => func,
        other => {
            return Err(anyhow!(
                "map() second argument must be a function, got {}",
                other.type_name()
            ));
        }
    };

    let mut out: Vec<Val> = Vec::with_capacity(list.len());
    for item in list.iter() {
        let res = call.call(std::slice::from_ref(item), ctx)?;
        out.push(res);
    }
    Ok(Val::List(out.into()))
}

/// filter - keep elements where predicate is truthy
/// Truthiness: false and nil are false; everything else true.
/// Accepts (receiver, function), including method sugar.
pub fn filter(args: &[Val], ctx: &mut VmContext) -> Result<Val> {
    if let [Val::Iterator(iter), func] = args {
        let predicate = expect_callable(func, "filter() second argument")?;
        let handle = filter_iterator_handle(iter.clone(), predicate)?;
        warn_legacy(
            "iter.filter(iterator)",
            "Returning materialised list for compatibility; prefer iterator.filter(...).collect()",
        );
        return collect_iterator_to_list(handle, ctx);
    }
    if matches!(args, [Val::List(_), _]) {
        warn_legacy(
            "iter.filter(list)",
            "Legacy eager filter is deprecated; use list.into_iter().filter(...).collect()",
        );
    }
    legacy_filter(args, ctx)
}

fn legacy_filter(args: &[Val], ctx: &mut VmContext) -> Result<Val> {
    let (list, func) = match args {
        [Val::List(l), f] => (l.clone(), f.clone()),
        _ => return Err(anyhow!("filter() expects (list, function)")),
    };

    let call = match func {
        Val::Closure(_) | Val::RustFunction(_) => func,
        other => {
            return Err(anyhow!(
                "filter() second argument must be a function, got {}",
                other.type_name()
            ));
        }
    };

    let mut out: Vec<Val> = Vec::with_capacity(list.len());
    for item in list.iter() {
        let res = call.call(std::slice::from_ref(item), ctx)?;
        if truthy(&res) {
            out.push(item.clone());
        }
    }
    Ok(Val::List(out.into()))
}

/// reduce - fold list with accumulator
/// Accepts (receiver, init, function)
pub fn reduce(args: &[Val], ctx: &mut VmContext) -> Result<Val> {
    if args.len() != 3 {
        return Err(anyhow!("reduce() expects 3 arguments: list|iterator, init, function"));
    }
    if let Val::Iterator(iter) = &args[0] {
        let func = expect_callable(&args[2], "reduce() third argument")?;
        let mut acc = args[1].clone();
        while let Some(value) = iter.next(ctx)? {
            let args = [acc, value];
            acc = func.call(&args, ctx)?;
        }
        return Ok(acc);
    }
    if matches!(args[0], Val::List(_)) {
        warn_legacy(
            "iter.reduce(list)",
            "Legacy eager reduce is deprecated; use list.into_iter().reduce(...)",
        );
    }
    legacy_reduce(args, ctx)
}

fn legacy_reduce(args: &[Val], ctx: &mut VmContext) -> Result<Val> {
    let list = match &args[0] {
        Val::List(l) => l.clone(),
        _ => return Err(anyhow!("reduce() first argument must be a list")),
    };
    let mut acc = args[1].clone();
    let func = match &args[2] {
        f @ Val::Closure(_) | f @ Val::RustFunction(_) => f.clone(),
        other => {
            return Err(anyhow!(
                "reduce() third argument must be a function, got {}",
                other.type_name()
            ));
        }
    };

    for item in list.iter() {
        acc = func.call(&[acc, item.clone()], ctx)?;
    }
    Ok(acc)
}

/// enumerate - 为序列添加索引
/// enumerate([1, 2, 3]) => [[0, 1], [1, 2], [2, 3]]
pub fn enumerate(args: &[Val], _: &mut VmContext) -> Result<Val> {
    if args.len() != 1 {
        return Err(anyhow!("enumerate expects 1 argument, got {}", args.len()));
    }

    match &args[0] {
        Val::List(list) => {
            let enumerated: Vec<Val> = list
                .iter()
                .enumerate()
                .map(|(i, v)| Val::List(vec![Val::Int(i as i64), v.clone()].into()))
                .collect();
            Ok(Val::List(enumerated.into()))
        }
        _ => Err(anyhow!("enumerate expects a list, got {:?}", args[0].type_name())),
    }
}

/// range - 生成整数范围
/// range(5) => [0, 1, 2, 3, 4]
/// range(2, 5) => [2, 3, 4]
/// range(0, 10, 2) => [0, 2, 4, 6, 8]
pub fn range(args: &[Val], _: &mut VmContext) -> Result<Val> {
    let (start, end, step) = match args.len() {
        1 => (0, extract_int(&args[0])?, 1),
        2 => (extract_int(&args[0])?, extract_int(&args[1])?, 1),
        3 => (extract_int(&args[0])?, extract_int(&args[1])?, extract_int(&args[2])?),
        _ => return Err(anyhow!("range expects 1-3 arguments, got {}", args.len())),
    };

    if step == 0 {
        return Err(anyhow!("range step cannot be zero"));
    }

    let mut result = Vec::new();
    let mut current = start;

    if step > 0 {
        while current < end {
            result.push(Val::Int(current));
            current += step;
        }
    } else if step < 0 {
        while current > end {
            result.push(Val::Int(current));
            current += step;
        }
    }

    Ok(Val::List(result.into()))
}

/// Helper function to extract integer from Val
fn extract_int(val: &Val) -> Result<i64> {
    match val {
        Val::Int(i) => Ok(*i),
        _ => Err(anyhow!("Expected integer, got {:?}", val)),
    }
}

/// zip - pair elements from two lists by index
/// zip([1,2], ["a","b","c"]) => [[1,"a"], [2,"b"]]
pub fn zip(args: &[Val], _: &mut VmContext) -> Result<Val> {
    if args.len() != 2 {
        return Err(anyhow!("zip expects 2 arguments: list1, list2"));
    }
    let a = match &args[0] {
        Val::List(l) => l,
        _ => return Err(anyhow!("zip first argument must be a list")),
    };
    let b = match &args[1] {
        Val::List(l) => l,
        _ => return Err(anyhow!("zip second argument must be a list")),
    };
    let len = std::cmp::min(a.len(), b.len());
    let mut out = Vec::with_capacity(len);
    for i in 0..len {
        out.push(Val::List(vec![a[i].clone(), b[i].clone()].into()));
    }
    Ok(Val::List(out.into()))
}

/// take - take first n elements from list
pub fn take(args: &[Val], _: &mut VmContext) -> Result<Val> {
    if args.len() != 2 {
        return Err(anyhow!("take expects 2 arguments: list, n"));
    }
    let list = match &args[0] {
        Val::List(l) => l,
        _ => return Err(anyhow!("take first argument must be a list")),
    };
    let n = extract_int(&args[1])?;
    if n <= 0 {
        return Ok(Val::List(Vec::<Val>::new().into()));
    }
    let end = std::cmp::min(list.len(), n as usize);
    Ok(Val::List(list[0..end].to_vec().into()))
}

/// skip - skip first n elements from list
pub fn skip(args: &[Val], _: &mut VmContext) -> Result<Val> {
    if args.len() != 2 {
        return Err(anyhow!("skip expects 2 arguments: list, n"));
    }
    let list = match &args[0] {
        Val::List(l) => l,
        _ => return Err(anyhow!("skip first argument must be a list")),
    };
    let n = extract_int(&args[1])?;
    if n <= 0 {
        return Ok(Val::List(list.clone()));
    }
    let start = std::cmp::min(list.len(), n as usize);
    Ok(Val::List(list[start..].to_vec().into()))
}

/// chain - concatenate two lists
pub fn chain(args: &[Val], _: &mut VmContext) -> Result<Val> {
    if args.len() != 2 {
        return Err(anyhow!("chain expects 2 arguments: list1, list2"));
    }
    let a = match &args[0] {
        Val::List(l) => l,
        _ => return Err(anyhow!("chain first argument must be a list")),
    };
    let b = match &args[1] {
        Val::List(l) => l,
        _ => return Err(anyhow!("chain second argument must be a list")),
    };
    let mut out = Vec::with_capacity(a.len() + b.len());
    out.extend(a.iter().cloned());
    out.extend(b.iter().cloned());
    Ok(Val::List(out.into()))
}

/// flatten - flatten one level of nesting in a list
pub fn flatten(args: &[Val], _: &mut VmContext) -> Result<Val> {
    if args.len() != 1 {
        return Err(anyhow!("flatten expects 1 argument: list"));
    }
    let list = match &args[0] {
        Val::List(l) => l,
        _ => return Err(anyhow!("flatten argument must be a list")),
    };
    let mut out: Vec<Val> = Vec::new();
    for item in list.iter() {
        match item {
            Val::List(inner) => out.extend(inner.iter().cloned()),
            other => out.push(other.clone()),
        }
    }
    Ok(Val::List(out.into()))
}

/// unique - remove duplicates (O(n^2), stable)
pub fn unique(args: &[Val], _: &mut VmContext) -> Result<Val> {
    if args.len() != 1 {
        return Err(anyhow!("unique expects 1 argument: list"));
    }
    let list = match &args[0] {
        Val::List(l) => &**l,
        _ => return Err(anyhow!("unique argument must be a list")),
    };
    let mut out: Vec<Val> = Vec::with_capacity(list.len());
    'outer: for v in list.iter() {
        for seen in out.iter() {
            if seen == v {
                continue 'outer;
            }
        }
        out.push(v.clone());
    }
    Ok(Val::List(out.into()))
}

/// chunk - split list into chunks of given positive size
pub fn chunk(args: &[Val], _: &mut VmContext) -> Result<Val> {
    if args.len() != 2 {
        return Err(anyhow!("chunk expects 2 arguments: list, size"));
    }
    let list = match &args[0] {
        Val::List(l) => &**l,
        _ => return Err(anyhow!("chunk first argument must be a list")),
    };
    let size = extract_int(&args[1])?;
    if size <= 0 {
        return Err(anyhow!("chunk size must be positive"));
    }
    let size = size as usize;
    let mut out: Vec<Val> = Vec::new();
    let mut i = 0usize;
    while i < list.len() {
        let end = std::cmp::min(i + size, list.len());
        out.push(Val::List(list[i..end].to_vec().into()));
        i = end;
    }
    Ok(Val::List(out.into()))
}

fn next(args: &[Val], ctx: &mut VmContext) -> Result<Val> {
    if args.len() != 1 {
        return Err(anyhow!("next() expects exactly 1 argument: iterator"));
    }
    let iter = expect_iterator(&args[0], "next()")?;
    Ok(iter.next(ctx)?.unwrap_or(Val::Nil))
}

fn collect(args: &[Val], ctx: &mut VmContext) -> Result<Val> {
    match args {
        [Val::Iterator(iter)] => collect_iterator_to_list(iter.clone(), ctx),
        [Val::Iterator(iter), Val::Str(target)] if target.as_ref() == "list" => {
            collect_iterator_to_list(iter.clone(), ctx)
        }
        [Val::Iterator(_), target] => Err(anyhow!("collect() unsupported target type {}", target.type_name())),
        _ => Err(anyhow!("collect() expects (iterator[, target_type])")),
    }
}

fn iterator_map_method(args: &[Val], _ctx: &mut VmContext) -> Result<Val> {
    if args.len() != 2 {
        return Err(anyhow!("Iterator.map expects (iterator, function)"));
    }
    let iter = expect_iterator(&args[0], "Iterator.map")?;
    let mapper = expect_callable(&args[1], "Iterator.map")?;
    let handle = map_iterator_handle(iter, mapper)?;
    Ok(Val::Iterator(handle))
}

fn iterator_filter_method(args: &[Val], _ctx: &mut VmContext) -> Result<Val> {
    if args.len() != 2 {
        return Err(anyhow!("Iterator.filter expects (iterator, function)"));
    }
    let iter = expect_iterator(&args[0], "Iterator.filter")?;
    let predicate = expect_callable(&args[1], "Iterator.filter")?;
    let handle = filter_iterator_handle(iter, predicate)?;
    Ok(Val::Iterator(handle))
}

fn iterator_reduce_method(args: &[Val], ctx: &mut VmContext) -> Result<Val> {
    if args.len() != 3 {
        return Err(anyhow!("Iterator.reduce expects (iterator, init, function)"));
    }
    let iter = expect_iterator(&args[0], "Iterator.reduce")?;
    let func = expect_callable(&args[2], "Iterator.reduce")?;
    let mut acc = args[1].clone();
    while let Some(value) = iter.next(ctx)? {
        let call_args = [acc, value];
        acc = func.call(&call_args, ctx)?;
    }
    Ok(acc)
}

fn iterator_next_method(args: &[Val], ctx: &mut VmContext) -> Result<Val> {
    if args.len() != 1 {
        return Err(anyhow!("Iterator.next expects exactly 1 argument"));
    }
    let iter = expect_iterator(&args[0], "Iterator.next")?;
    Ok(iter.next(ctx)?.unwrap_or(Val::Nil))
}

fn iterator_collect_method(args: &[Val], ctx: &mut VmContext) -> Result<Val> {
    match args {
        [val] => {
            let iter = expect_iterator(val, "Iterator.collect")?;
            collect_iterator_to_list(iter, ctx)
        }
        [val, target] => {
            let iter = expect_iterator(val, "Iterator.collect")?;
            collect(&[Val::Iterator(iter), target.clone()], ctx)
        }
        _ => Err(anyhow!("Iterator.collect expects (iterator[, target_type])")),
    }
}

#[cfg(test)]
mod tests {

    use crate::register_stdlib_modules;
    use anyhow::Result;
    use lkr_core::{
        stmt::stmt_parser::StmtParser,
        token::Tokenizer,
        val::Val,
        vm::{Vm, VmContext},
    };
    use std::sync::Arc;

    fn run(source: &str) -> Result<Val> {
        let tokens = Tokenizer::tokenize(source)?;
        let mut parser = StmtParser::new(&tokens);
        let program = parser.parse_program()?;

        let mut registry = lkr_core::module::ModuleRegistry::new();
        register_stdlib_modules(&mut registry)?;
        let resolver = Arc::new(lkr_core::stmt::ModuleResolver::with_registry(registry));
        let mut env = VmContext::new().with_resolver(resolver);
        let mut vm = Vm::new();
        program.execute_with_vm(&mut vm, &mut env)
    }

    #[test]
    fn test_iter_zip() -> Result<()> {
        let v = run("import iter; return iter.zip([1,2], [\"a\",\"b\",\"c\"]);")?;
        assert_eq!(
            v,
            Val::List(Arc::from(vec![
                Val::List(vec![Val::Int(1), Val::Str("a".into())].into()),
                Val::List(vec![Val::Int(2), Val::Str("b".into())].into()),
            ]))
        );
        Ok(())
    }

    #[test]
    fn test_iter_take_skip_chain_flatten_unique_chunk() -> Result<()> {
        // take
        assert_eq!(
            run("import iter; return iter.take([1,2,3,4], 2);")?,
            Val::List(Arc::from(vec![Val::Int(1), Val::Int(2)]))
        );
        assert_eq!(
            run("import iter; return iter.take([1,2], 0);")?,
            Val::List(Arc::from(vec![]))
        );
        // skip
        assert_eq!(
            run("import iter; return iter.skip([1,2,3,4], 2);")?,
            Val::List(Arc::from(vec![Val::Int(3), Val::Int(4)]))
        );
        assert_eq!(
            run("import iter; return iter.skip([1,2], 10);")?,
            Val::List(Arc::from(vec![]))
        );
        // chain
        assert_eq!(
            run("import iter; return iter.chain([1,2], [3,4]);")?,
            Val::List(Arc::from(vec![Val::Int(1), Val::Int(2), Val::Int(3), Val::Int(4)]))
        );
        // flatten
        assert_eq!(
            run("import iter; return iter.flatten([[1,2],[3],[4]]);")?,
            Val::List(Arc::from(vec![Val::Int(1), Val::Int(2), Val::Int(3), Val::Int(4)]))
        );
        // unique
        assert_eq!(
            run("import iter; return iter.unique([1,1,2,2,3]);")?,
            Val::List(Arc::from(vec![Val::Int(1), Val::Int(2), Val::Int(3)]))
        );
        // chunk
        assert_eq!(
            run("import iter; return iter.chunk([1,2,3,4,5], 2);")?,
            Val::List(Arc::from(vec![
                Val::List(Arc::from(vec![Val::Int(1), Val::Int(2)])),
                Val::List(Arc::from(vec![Val::Int(3), Val::Int(4)])),
                Val::List(Arc::from(vec![Val::Int(5)])),
            ]))
        );
        Ok(())
    }

    #[test]
    fn test_iterator_pipeline_collects() -> Result<()> {
        let value = run("
            import iter;
            let xs = [1, 2, 3, 4];
            let result = xs
                .into_iter()
                .map(fn(x) => x * x)
                .filter(fn(x) => x % 2 == 0)
                .collect();
            return result;
            ")?;
        assert_eq!(value, Val::List(Arc::from(vec![Val::Int(4), Val::Int(16)])));
        Ok(())
    }

    #[test]
    fn test_iterator_reduce() -> Result<()> {
        let value = run("
            import iter;
            let xs = [1, 2, 3];
            let sum = xs.into_iter().reduce(0, fn(acc, x) => acc + x);
            return sum;
            ")?;
        assert_eq!(value, Val::Int(6));
        Ok(())
    }
}
