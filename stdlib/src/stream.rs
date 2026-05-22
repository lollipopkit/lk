use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

use anyhow::{Result, anyhow, bail};
use dashmap::DashMap;
use lk_core::{
    module::{Module, ModuleRegistry},
    rt::{self, RuntimePayload},
    val::{CallableValue, HeapStore, HeapValue, RuntimeVal, StreamCursorValue, StreamValue, Type, TypedList, Val},
    vm::{
        NativeArgs32, NativeEntry32, NativeFunction32, NativeRuntime32, call_runtime_callable32_runtime,
        copy_runtime_value, runtime_value_to_callable32,
    },
};
use once_cell::sync::Lazy;

#[derive(Debug)]
pub struct StreamModule {
    functions: HashMap<String, Val>,
}

impl Default for StreamModule {
    fn default() -> Self {
        Self::new()
    }
}

static NEXT_STREAM_ID: AtomicU64 = AtomicU64::new(1);
static NEXT_CURSOR_ID: AtomicU64 = AtomicU64::new(1);

static STREAMS: Lazy<DashMap<u64, Arc<StreamSpec>>> = Lazy::new(DashMap::new);
type CursorBox = Box<dyn StreamCursor + Send>;
type CursorHandle = Arc<Mutex<CursorBox>>;
static CURSORS: Lazy<DashMap<u64, CursorHandle>> = Lazy::new(DashMap::new);
static CURSOR_INFO: Lazy<DashMap<u64, CursorInfo>> = Lazy::new(DashMap::new);

#[derive(Debug, Clone, Default)]
struct CursorInfo {
    channel_id: Option<u64>,
}

#[derive(Debug, Clone)]
enum StreamSpec {
    FromList(Vec<RuntimeVal>),
    Range {
        start: i64,
        end: Option<i64>,
        step: i64,
    },
    Repeat(RuntimeVal),
    Iterate {
        seed: RuntimeVal,
        func: RuntimeVal,
    },
    FromChannel {
        channel_id: u64,
    },
    Map {
        upstream: Arc<StreamSpec>,
        func: RuntimeVal,
    },
    Filter {
        upstream: Arc<StreamSpec>,
        func: RuntimeVal,
    },
    Take {
        upstream: Arc<StreamSpec>,
        n: i64,
    },
    Skip {
        upstream: Arc<StreamSpec>,
        n: i64,
    },
    Chain {
        left: Arc<StreamSpec>,
        right: Arc<StreamSpec>,
    },
}

trait StreamCursor {
    fn next(&mut self, runtime: &mut NativeRuntime32<'_>) -> Result<Option<RuntimeVal>>;
}

impl StreamSpec {
    fn open_cursor(&self) -> Box<dyn StreamCursor + Send> {
        match self.clone() {
            StreamSpec::FromList(data) => Box::new(FromListCursor { data, index: 0 }),
            StreamSpec::Range { start, end, step } => Box::new(RangeCursor {
                current: start,
                end,
                step,
            }),
            StreamSpec::Repeat(value) => Box::new(RepeatCursor { value }),
            StreamSpec::Iterate { seed, func } => Box::new(IterateCursor {
                current: seed,
                func,
                first: true,
            }),
            StreamSpec::FromChannel { channel_id } => Box::new(ChannelCursor { channel_id }),
            StreamSpec::Map { upstream, func } => Box::new(MapCursor {
                upstream: upstream.open_cursor(),
                func,
            }),
            StreamSpec::Filter { upstream, func } => Box::new(FilterCursor {
                upstream: upstream.open_cursor(),
                func,
            }),
            StreamSpec::Take { upstream, n } => Box::new(TakeCursor {
                upstream: upstream.open_cursor(),
                remaining: n,
            }),
            StreamSpec::Skip { upstream, n } => Box::new(SkipCursor {
                upstream: upstream.open_cursor(),
                to_skip: n,
            }),
            StreamSpec::Chain { left, right } => Box::new(ChainCursor {
                left: left.open_cursor(),
                right: right.open_cursor(),
                left_exhausted: false,
            }),
        }
    }
}

#[derive(Debug)]
struct FromListCursor {
    data: Vec<RuntimeVal>,
    index: usize,
}

impl StreamCursor for FromListCursor {
    fn next(&mut self, _runtime: &mut NativeRuntime32<'_>) -> Result<Option<RuntimeVal>> {
        let Some(value) = self.data.get(self.index).cloned() else {
            return Ok(None);
        };
        self.index += 1;
        Ok(Some(value))
    }
}

#[derive(Debug)]
struct RangeCursor {
    current: i64,
    end: Option<i64>,
    step: i64,
}

impl StreamCursor for RangeCursor {
    fn next(&mut self, _runtime: &mut NativeRuntime32<'_>) -> Result<Option<RuntimeVal>> {
        if self.step == 0 {
            bail!("range step cannot be zero");
        }
        if let Some(end) = self.end
            && ((self.step > 0 && self.current >= end) || (self.step < 0 && self.current <= end))
        {
            return Ok(None);
        }
        let value = self.current;
        self.current += self.step;
        Ok(Some(RuntimeVal::Int(value)))
    }
}

#[derive(Debug)]
struct RepeatCursor {
    value: RuntimeVal,
}

impl StreamCursor for RepeatCursor {
    fn next(&mut self, _runtime: &mut NativeRuntime32<'_>) -> Result<Option<RuntimeVal>> {
        Ok(Some(self.value.clone()))
    }
}

#[derive(Debug)]
struct IterateCursor {
    current: RuntimeVal,
    func: RuntimeVal,
    first: bool,
}

impl StreamCursor for IterateCursor {
    fn next(&mut self, runtime: &mut NativeRuntime32<'_>) -> Result<Option<RuntimeVal>> {
        if self.first {
            self.first = false;
            return Ok(Some(self.current.clone()));
        }
        let next = call_runtime_callable_value(
            &self.func,
            std::slice::from_ref(&self.current),
            runtime,
            "stream.iterate",
        )?;
        self.current = next.clone();
        Ok(Some(next))
    }
}

struct MapCursor {
    upstream: Box<dyn StreamCursor + Send>,
    func: RuntimeVal,
}

impl StreamCursor for MapCursor {
    fn next(&mut self, runtime: &mut NativeRuntime32<'_>) -> Result<Option<RuntimeVal>> {
        let Some(value) = self.upstream.next(runtime)? else {
            return Ok(None);
        };
        call_runtime_callable_value(&self.func, &[value], runtime, "stream.map").map(Some)
    }
}

struct FilterCursor {
    upstream: Box<dyn StreamCursor + Send>,
    func: RuntimeVal,
}

impl StreamCursor for FilterCursor {
    fn next(&mut self, runtime: &mut NativeRuntime32<'_>) -> Result<Option<RuntimeVal>> {
        loop {
            let Some(value) = self.upstream.next(runtime)? else {
                return Ok(None);
            };
            let keep = call_runtime_callable_value(&self.func, std::slice::from_ref(&value), runtime, "stream.filter")?;
            if truthy(&keep) {
                return Ok(Some(value));
            }
        }
    }
}

struct TakeCursor {
    upstream: Box<dyn StreamCursor + Send>,
    remaining: i64,
}

impl StreamCursor for TakeCursor {
    fn next(&mut self, runtime: &mut NativeRuntime32<'_>) -> Result<Option<RuntimeVal>> {
        if self.remaining <= 0 {
            return Ok(None);
        }
        let value = self.upstream.next(runtime)?;
        if value.is_some() {
            self.remaining -= 1;
        }
        Ok(value)
    }
}

struct SkipCursor {
    upstream: Box<dyn StreamCursor + Send>,
    to_skip: i64,
}

impl StreamCursor for SkipCursor {
    fn next(&mut self, runtime: &mut NativeRuntime32<'_>) -> Result<Option<RuntimeVal>> {
        while self.to_skip > 0 {
            if self.upstream.next(runtime)?.is_none() {
                return Ok(None);
            }
            self.to_skip -= 1;
        }
        self.upstream.next(runtime)
    }
}

struct ChainCursor {
    left: Box<dyn StreamCursor + Send>,
    right: Box<dyn StreamCursor + Send>,
    left_exhausted: bool,
}

impl StreamCursor for ChainCursor {
    fn next(&mut self, runtime: &mut NativeRuntime32<'_>) -> Result<Option<RuntimeVal>> {
        if !self.left_exhausted {
            if let Some(value) = self.left.next(runtime)? {
                return Ok(Some(value));
            }
            self.left_exhausted = true;
        }
        self.right.next(runtime)
    }
}

#[derive(Debug)]
struct ChannelCursor {
    channel_id: u64,
}

impl StreamCursor for ChannelCursor {
    fn next(&mut self, runtime: &mut NativeRuntime32<'_>) -> Result<Option<RuntimeVal>> {
        match rt::with_runtime(|runtime| runtime.try_recv(self.channel_id))? {
            Some((true, value)) => Ok(Some(runtime_payload_into_value(value, runtime.heap_mut())?)),
            Some((false, _)) | None => Ok(None),
        }
    }
}

impl StreamModule {
    pub fn new() -> Self {
        let mut functions = HashMap::new();

        register_native(&mut functions, "from_list", from_list32, 1);
        register_native(&mut functions, "range", range32, NativeEntry32::VARIADIC);
        register_native(&mut functions, "iterate", iterate32, 2);
        register_native(&mut functions, "repeat", repeat32, 1);
        register_native(&mut functions, "from_channel", from_channel32, 1);
        register_native(&mut functions, "map", map32, 2);
        register_native(&mut functions, "filter", filter32, 2);
        register_native(&mut functions, "take", take32, 2);
        register_native(&mut functions, "skip", skip32, 2);
        register_native(&mut functions, "chain", chain32, 2);
        register_native(&mut functions, "subscribe", subscribe32, 1);
        register_native(&mut functions, "next", next32, 1);
        register_native(&mut functions, "collect", collect32, NativeEntry32::VARIADIC);
        register_native(&mut functions, "next_block", next_block32, NativeEntry32::VARIADIC);
        register_native(
            &mut functions,
            "collect_block",
            collect_block32,
            NativeEntry32::VARIADIC,
        );

        Self { functions }
    }
}

impl Module for StreamModule {
    fn name(&self) -> &str {
        "stream"
    }

    fn description(&self) -> &str {
        "Lazy, cold stream utilities"
    }

    fn register(&self, _registry: &mut ModuleRegistry) -> Result<()> {
        Ok(())
    }

    fn exports(&self) -> HashMap<String, Val> {
        self.functions.clone()
    }
}

fn register_native(
    functions: &mut HashMap<String, Val>,
    name: &str,
    function: fn(NativeArgs32<'_>, &mut NativeRuntime32<'_>) -> Result<RuntimeVal>,
    arity: u16,
) {
    functions.insert(
        name.to_string(),
        Val::runtime_native32(NativeFunction32::Plain(function), arity),
    );
}

fn from_list32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 1, "stream.from_list")?;
    let values = list_items(&args.as_slice()[0], runtime.heap_mut(), "stream.from_list argument")?;
    create_stream(StreamSpec::FromList(values), Type::Any, runtime.heap_mut())
}

fn range32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
    let values = args.as_slice();
    let (start, end, step) = match values {
        [end] => (0, Some(int_arg(end, "stream.range end")?), 1),
        [start, end] => (
            int_arg(start, "stream.range start")?,
            Some(int_arg(end, "stream.range end")?),
            1,
        ),
        [start, end, step] => (
            int_arg(start, "stream.range start")?,
            Some(int_arg(end, "stream.range end")?),
            int_arg(step, "stream.range step")?,
        ),
        _ => bail!("stream.range expects 1-3 arguments"),
    };
    create_stream(StreamSpec::Range { start, end, step }, Type::Int, runtime.heap_mut())
}

fn iterate32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 2, "stream.iterate")?;
    let values = args.as_slice();
    ensure_runtime_callable(&values[1], runtime, "stream.iterate function")?;
    create_stream(
        StreamSpec::Iterate {
            seed: values[0].clone(),
            func: values[1].clone(),
        },
        Type::Any,
        runtime.heap_mut(),
    )
}

fn repeat32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 1, "stream.repeat")?;
    create_stream(
        StreamSpec::Repeat(args.as_slice()[0].clone()),
        Type::Any,
        runtime.heap_mut(),
    )
}

fn from_channel32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 1, "stream.from_channel")?;
    let channel = channel_arg(&args.as_slice()[0], runtime.heap(), "stream.from_channel argument")?;
    create_stream(
        StreamSpec::FromChannel { channel_id: channel.id },
        channel.inner_type.clone(),
        runtime.heap_mut(),
    )
}

fn map32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 2, "stream.map")?;
    let values = args.as_slice();
    ensure_runtime_callable(&values[1], runtime, "stream.map function")?;
    let upstream = get_stream_spec(stream_id_arg(&values[0], runtime.heap(), "stream.map stream")?)?;
    create_stream(
        StreamSpec::Map {
            upstream,
            func: values[1].clone(),
        },
        Type::Any,
        runtime.heap_mut(),
    )
}

fn filter32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 2, "stream.filter")?;
    let values = args.as_slice();
    ensure_runtime_callable(&values[1], runtime, "stream.filter function")?;
    let upstream = get_stream_spec(stream_id_arg(&values[0], runtime.heap(), "stream.filter stream")?)?;
    create_stream(
        StreamSpec::Filter {
            upstream,
            func: values[1].clone(),
        },
        Type::Any,
        runtime.heap_mut(),
    )
}

fn take32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 2, "stream.take")?;
    let values = args.as_slice();
    let upstream = get_stream_spec(stream_id_arg(&values[0], runtime.heap(), "stream.take stream")?)?;
    let n = int_arg(&values[1], "stream.take count")?;
    create_stream(StreamSpec::Take { upstream, n }, Type::Any, runtime.heap_mut())
}

fn skip32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 2, "stream.skip")?;
    let values = args.as_slice();
    let upstream = get_stream_spec(stream_id_arg(&values[0], runtime.heap(), "stream.skip stream")?)?;
    let n = int_arg(&values[1], "stream.skip count")?;
    create_stream(StreamSpec::Skip { upstream, n }, Type::Any, runtime.heap_mut())
}

fn chain32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 2, "stream.chain")?;
    let values = args.as_slice();
    let left = get_stream_spec(stream_id_arg(&values[0], runtime.heap(), "stream.chain left")?)?;
    let right = get_stream_spec(stream_id_arg(&values[1], runtime.heap(), "stream.chain right")?)?;
    create_stream(StreamSpec::Chain { left, right }, Type::Any, runtime.heap_mut())
}

fn subscribe32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 1, "stream.subscribe")?;
    create_cursor(
        stream_id_arg(&args.as_slice()[0], runtime.heap(), "stream.subscribe argument")?,
        runtime.heap_mut(),
    )
}

fn next32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 1, "stream.next")?;
    let cursor_id = cursor_id_arg(&args.as_slice()[0], runtime.heap(), "stream.next argument")?;
    next_cursor(cursor_id, runtime)
}

fn collect32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
    let (cursor_id, limit) = cursor_and_limit(args.as_slice(), runtime, "stream.collect")?;
    collect_cursor(cursor_id, limit, runtime)
}

fn next_block32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
    let values = args.as_slice();
    if values.is_empty() || values.len() > 2 {
        bail!("stream.next_block expects (cursor[, timeout_ms])");
    }
    let cursor_id = cursor_id_arg(&values[0], runtime.heap(), "stream.next_block cursor")?;
    let timeout_ms = match values.get(1) {
        Some(value) => Some(int_arg(value, "stream.next_block timeout_ms")?),
        None => None,
    };
    next_block_cursor(cursor_id, timeout_ms, runtime)
}

fn collect_block32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
    let (cursor_id, limit, timeout_ms) = cursor_limit_timeout(args.as_slice(), runtime, "stream.collect_block")?;
    collect_block_cursor(cursor_id, limit, timeout_ms, runtime)
}

fn create_stream(spec: StreamSpec, inner_type: Type, heap: &mut HeapStore) -> Result<RuntimeVal> {
    let id = NEXT_STREAM_ID.fetch_add(1, Ordering::Relaxed);
    STREAMS.insert(id, Arc::new(spec));
    Ok(RuntimeVal::Obj(
        heap.alloc(HeapValue::Stream(Arc::new(StreamValue { id, inner_type }))),
    ))
}

fn create_cursor(stream_id: u64, heap: &mut HeapStore) -> Result<RuntimeVal> {
    let spec = get_stream_spec(stream_id)?;
    let cursor = spec.open_cursor();
    let id = NEXT_CURSOR_ID.fetch_add(1, Ordering::Relaxed);
    CURSORS.insert(id, Arc::new(Mutex::new(cursor)));
    let channel_id = match spec.as_ref() {
        StreamSpec::FromChannel { channel_id } => Some(*channel_id),
        _ => None,
    };
    CURSOR_INFO.insert(id, CursorInfo { channel_id });
    Ok(RuntimeVal::Obj(heap.alloc(HeapValue::StreamCursor(Arc::new(
        StreamCursorValue { id, stream_id },
    )))))
}

fn get_stream_spec(id: u64) -> Result<Arc<StreamSpec>> {
    STREAMS
        .get(&id)
        .map(|entry| entry.value().clone())
        .ok_or_else(|| anyhow!("Stream not found: {}", id))
}

fn next_cursor(cursor_id: u64, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
    let cursor = cursor_handle(cursor_id)?;
    let value = {
        let mut guard = cursor.lock().map_err(|_| anyhow!("cursor mutex poisoned"))?;
        guard.next(runtime)?
    };
    match value {
        Some(value) => runtime_list(vec![RuntimeVal::Bool(true), value], runtime.heap_mut()),
        None => runtime_list(vec![RuntimeVal::Bool(false), RuntimeVal::Nil], runtime.heap_mut()),
    }
}

fn collect_cursor(cursor_id: u64, limit: Option<i64>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
    let cursor = cursor_handle(cursor_id)?;
    let mut out = Vec::new();
    let mut taken = 0i64;
    loop {
        if let Some(limit) = limit
            && taken >= limit
        {
            break;
        }
        let value = {
            let mut guard = cursor.lock().map_err(|_| anyhow!("cursor mutex poisoned"))?;
            guard.next(runtime)?
        };
        let Some(value) = value else {
            break;
        };
        out.push(value);
        taken += 1;
    }
    runtime_list(out, runtime.heap_mut())
}

fn next_block_cursor(cursor_id: u64, timeout_ms: Option<i64>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
    let info = CURSOR_INFO
        .get(&cursor_id)
        .map(|entry| entry.value().clone())
        .unwrap_or_default();
    let Some(channel_id) = info.channel_id else {
        return next_cursor(cursor_id, runtime);
    };
    let (ok, value) = recv_channel_blocking(channel_id, timeout_ms)?;
    let value = runtime_payload_into_value(value, runtime.heap_mut())?;
    runtime_list(vec![RuntimeVal::Bool(ok), value], runtime.heap_mut())
}

fn collect_block_cursor(
    cursor_id: u64,
    limit: Option<i64>,
    timeout_ms: Option<i64>,
    runtime: &mut NativeRuntime32<'_>,
) -> Result<RuntimeVal> {
    let info = CURSOR_INFO
        .get(&cursor_id)
        .map(|entry| entry.value().clone())
        .unwrap_or_default();
    let Some(channel_id) = info.channel_id else {
        return collect_cursor(cursor_id, limit, runtime);
    };
    let mut out = Vec::new();
    let mut taken = 0i64;
    loop {
        if let Some(limit) = limit
            && taken >= limit
        {
            break;
        }
        let Some((ok, value)) = recv_channel_blocking_optional(channel_id, timeout_ms)? else {
            break;
        };
        if !ok {
            break;
        }
        out.push(runtime_payload_into_value(value, runtime.heap_mut())?);
        taken += 1;
    }
    runtime_list(out, runtime.heap_mut())
}

fn recv_channel_blocking(channel_id: u64, timeout_ms: Option<i64>) -> Result<(bool, RuntimePayload)> {
    Ok(recv_channel_blocking_optional(channel_id, timeout_ms)?.unwrap_or((false, RuntimePayload::nil())))
}

fn recv_channel_blocking_optional(channel_id: u64, timeout_ms: Option<i64>) -> Result<Option<(bool, RuntimePayload)>> {
    use std::time::Duration;
    let value = rt::with_runtime(|runtime| match timeout_ms {
        Some(ms) if ms > 0 => {
            let future = runtime.recv_async(channel_id);
            match runtime.block_on(async move { tokio::time::timeout(Duration::from_millis(ms as u64), future).await })
            {
                Ok(result) => result.map(Some),
                Err(_) => Ok(None),
            }
        }
        _ => runtime.block_on(runtime.recv_async(channel_id)).map(Some),
    })?;
    Ok(value)
}

fn runtime_payload_into_value(payload: RuntimePayload, heap: &mut HeapStore) -> Result<RuntimeVal> {
    let mut payload_heap = payload.heap;
    copy_runtime_value(&payload.value, &mut payload_heap, heap)
}

fn cursor_handle(cursor_id: u64) -> Result<CursorHandle> {
    CURSORS
        .get(&cursor_id)
        .map(|entry| entry.value().clone())
        .ok_or_else(|| anyhow!("Cursor not found: {}", cursor_id))
}

fn cursor_and_limit(
    values: &[RuntimeVal],
    runtime: &mut NativeRuntime32<'_>,
    context: &str,
) -> Result<(u64, Option<i64>)> {
    match values {
        [value] if is_stream(value, runtime.heap()) => {
            let cursor = create_cursor(stream_id_arg(value, runtime.heap(), context)?, runtime.heap_mut())?;
            Ok((cursor_id_arg(&cursor, runtime.heap(), context)?, None))
        }
        [value, limit] if is_stream(value, runtime.heap()) => {
            let cursor = create_cursor(stream_id_arg(value, runtime.heap(), context)?, runtime.heap_mut())?;
            Ok((
                cursor_id_arg(&cursor, runtime.heap(), context)?,
                Some(int_arg(limit, "stream.collect limit")?),
            ))
        }
        [value] if is_cursor(value, runtime.heap()) => Ok((cursor_id_arg(value, runtime.heap(), context)?, None)),
        [value, limit] if is_cursor(value, runtime.heap()) => Ok((
            cursor_id_arg(value, runtime.heap(), context)?,
            Some(int_arg(limit, "stream.collect limit")?),
        )),
        _ => bail!("{context} expects (stream|cursor[, n])"),
    }
}

fn cursor_limit_timeout(
    values: &[RuntimeVal],
    runtime: &mut NativeRuntime32<'_>,
    context: &str,
) -> Result<(u64, Option<i64>, Option<i64>)> {
    let (cursor_id, limit) = cursor_and_limit(values.get(..values.len().min(2)).unwrap_or(values), runtime, context)?;
    let timeout_ms = match values.get(2) {
        Some(value) => Some(int_arg(value, "stream.collect_block timeout_ms")?),
        None => None,
    };
    if values.len() > 3 {
        bail!("{context} expects (stream|cursor[, n][, timeout_ms])");
    }
    Ok((cursor_id, limit, timeout_ms))
}

fn stream_id_arg(value: &RuntimeVal, heap: &HeapStore, context: &str) -> Result<u64> {
    let RuntimeVal::Obj(handle) = value else {
        bail!("{context} must be a Stream");
    };
    match heap
        .get(*handle)
        .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?
    {
        HeapValue::Stream(stream) => Ok(stream.id),
        other => Err(anyhow!("{context} must be a Stream, got {}", other.type_name())),
    }
}

fn cursor_id_arg(value: &RuntimeVal, heap: &HeapStore, context: &str) -> Result<u64> {
    let RuntimeVal::Obj(handle) = value else {
        bail!("{context} must be a StreamCursor");
    };
    match heap
        .get(*handle)
        .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?
    {
        HeapValue::StreamCursor(cursor) => Ok(cursor.id),
        other => Err(anyhow!("{context} must be a StreamCursor, got {}", other.type_name())),
    }
}

fn channel_arg(value: &RuntimeVal, heap: &HeapStore, context: &str) -> Result<Arc<lk_core::val::ChannelValue>> {
    let RuntimeVal::Obj(handle) = value else {
        bail!("{context} must be a Channel");
    };
    match heap
        .get(*handle)
        .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?
    {
        HeapValue::Channel(channel) => Ok(channel.clone()),
        other => Err(anyhow!("{context} must be a Channel, got {}", other.type_name())),
    }
}

fn is_stream(value: &RuntimeVal, heap: &HeapStore) -> bool {
    matches!(value, RuntimeVal::Obj(handle) if matches!(heap.get(*handle), Some(HeapValue::Stream(_))))
}

fn is_cursor(value: &RuntimeVal, heap: &HeapStore) -> bool {
    matches!(value, RuntimeVal::Obj(handle) if matches!(heap.get(*handle), Some(HeapValue::StreamCursor(_))))
}

fn list_items(value: &RuntimeVal, heap: &mut HeapStore, context: &str) -> Result<Vec<RuntimeVal>> {
    let RuntimeVal::Obj(handle) = value else {
        bail!("{context} must be a List");
    };
    let value = heap
        .get(*handle)
        .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?
        .clone();
    match value {
        HeapValue::List(list) => Ok(list.materialize_mixed(heap)),
        other => Err(anyhow!("{context} must be a List, got {}", other.type_name())),
    }
}

fn runtime_list(values: Vec<RuntimeVal>, heap: &mut HeapStore) -> Result<RuntimeVal> {
    Ok(RuntimeVal::Obj(
        heap.alloc(HeapValue::List(TypedList::from_runtime_values(values, heap))),
    ))
}

fn int_arg(value: &RuntimeVal, context: &str) -> Result<i64> {
    match value {
        RuntimeVal::Int(value) => Ok(*value),
        _ => Err(anyhow!("{context} must be an integer")),
    }
}

fn expect_arity(args: NativeArgs32<'_>, expected: usize, name: &str) -> Result<()> {
    if args.len() == expected {
        Ok(())
    } else {
        Err(anyhow!(
            "{name} expects exactly {expected} argument{}",
            if expected == 1 { "" } else { "s" }
        ))
    }
}

fn truthy(value: &RuntimeVal) -> bool {
    !matches!(value, RuntimeVal::Nil | RuntimeVal::Bool(false))
}

fn ensure_runtime_callable(value: &RuntimeVal, runtime: &NativeRuntime32<'_>, context: &str) -> Result<()> {
    runtime_callable(value, runtime, context).map(|_| ())
}

fn call_runtime_callable_value(
    callable: &RuntimeVal,
    args: &[RuntimeVal],
    runtime: &mut NativeRuntime32<'_>,
    context: &str,
) -> Result<RuntimeVal> {
    let callable = runtime_callable(callable, runtime, context)?;
    let (heap, ctx) = runtime.heap_ctx_mut();
    call_runtime_callable32_runtime(&callable, NativeArgs32::new(args), heap, ctx)
}

fn runtime_callable(
    value: &RuntimeVal,
    runtime: &NativeRuntime32<'_>,
    context: &str,
) -> Result<lk_core::vm::RuntimeCallable32> {
    let RuntimeVal::Obj(handle) = value else {
        bail!("{context} must be a runtime callable");
    };
    let callable = runtime
        .heap()
        .get(*handle)
        .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?;
    match callable {
        HeapValue::Callable(CallableValue::Runtime32(function)) => Ok(function.as_ref().clone()),
        HeapValue::Callable(CallableValue::Closure { .. }) => {
            let module = runtime
                .module()
                .ok_or_else(|| anyhow!("{context} requires Module32 execution context"))?;
            runtime_value_to_callable32(value, runtime.heap(), &runtime.globals(), Arc::new((*module).clone()))
                .ok_or_else(|| anyhow!("{context} closure could not be materialized"))
        }
        _ => bail!("{context} must be a runtime callable"),
    }
}
