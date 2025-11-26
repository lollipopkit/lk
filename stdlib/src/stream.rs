use anyhow::{Result, anyhow};
use dashmap::DashMap;
use lkr_core::{
    module,
    module::Module,
    rt,
    val::{StreamValue, Type, Val, methods::register_method},
    vm::VmContext,
};
use once_cell::sync::Lazy;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Debug)]
pub struct StreamModule {
    functions: HashMap<String, Val>,
}

impl Default for StreamModule {
    fn default() -> Self {
        Self::new()
    }
}

// Global registries for streams and cursors
static NEXT_STREAM_ID: AtomicU64 = AtomicU64::new(1);
static NEXT_CURSOR_ID: AtomicU64 = AtomicU64::new(1);

static STREAMS: Lazy<DashMap<u64, Arc<StreamSpec>>> = Lazy::new(DashMap::new);
// Reduce type complexity for cursor registry
type CursorBox = Box<dyn StreamCursor + Send>;
type CursorHandle = Arc<Mutex<CursorBox>>;
type CursorMap = DashMap<u64, CursorHandle>;
static CURSORS: Lazy<CursorMap> = Lazy::new(DashMap::new);

#[derive(Debug, Clone, Default)]
struct CursorInfo {
    channel_id: Option<u64>,
}

static CURSOR_INFO: Lazy<DashMap<u64, CursorInfo>> = Lazy::new(DashMap::new);

// Stream specification: cold (multi-consumer) description that can open independent cursors
#[derive(Debug, Clone)]
enum StreamSpec {
    FromList(Arc<[Val]>),
    Range {
        start: i64,
        end: Option<i64>,
        step: i64,
    },
    Repeat(Val),
    Iterate {
        seed: Val,
        func: Val,
    }, // infinite: v0=seed, then v_{n+1}=func(v_n)
    FromChannel {
        channel_id: u64,
    },
    Map {
        upstream: Arc<StreamSpec>,
        func: Val,
    },
    Filter {
        upstream: Arc<StreamSpec>,
        func: Val,
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
    fn next(&mut self, ctx: &mut VmContext) -> Result<Option<Val>>;
}

impl StreamSpec {
    fn open_cursor(&self) -> Box<dyn StreamCursor + Send> {
        match self.clone() {
            StreamSpec::FromList(data) => Box::new(FromListCursor { data, idx: 0 }),
            StreamSpec::Range { start, end, step } => Box::new(RangeCursor { cur: start, end, step }),
            StreamSpec::Repeat(v) => Box::new(RepeatCursor { value: v }),
            StreamSpec::Iterate { seed, func } => Box::new(IterateCursor {
                cur: seed,
                func,
                first: true,
            }),
            StreamSpec::FromChannel { channel_id } => Box::new(ChannelCursor { channel_id }),
            StreamSpec::Map { upstream, func } => {
                let upstream_cursor = upstream.open_cursor();
                Box::new(MapCursor {
                    upstream: upstream_cursor,
                    func,
                })
            }
            StreamSpec::Filter { upstream, func } => {
                let upstream_cursor = upstream.open_cursor();
                Box::new(FilterCursor {
                    upstream: upstream_cursor,
                    func,
                })
            }
            StreamSpec::Take { upstream, n } => {
                let upstream_cursor = upstream.open_cursor();
                Box::new(TakeCursor {
                    upstream: upstream_cursor,
                    remaining: n,
                })
            }
            StreamSpec::Skip { upstream, n } => {
                let upstream_cursor = upstream.open_cursor();
                // Skipping performed lazily on first next()
                Box::new(SkipCursor {
                    upstream: upstream_cursor,
                    to_skip: n,
                })
            }
            StreamSpec::Chain { left, right } => {
                let left_cursor = left.open_cursor();
                let right_cursor = right.open_cursor();
                Box::new(ChainCursor {
                    left: left_cursor,
                    right: right_cursor,
                    left_exhausted: false,
                })
            }
        }
    }
}

// Concrete cursors
#[derive(Debug)]
struct FromListCursor {
    data: Arc<[Val]>,
    idx: usize,
}
impl StreamCursor for FromListCursor {
    fn next(&mut self, _ctx: &mut VmContext) -> Result<Option<Val>> {
        if self.idx >= self.data.len() {
            return Ok(None);
        }
        let v = self.data[self.idx].clone();
        self.idx += 1;
        Ok(Some(v))
    }
}

#[derive(Debug)]
struct RangeCursor {
    cur: i64,
    end: Option<i64>,
    step: i64,
}
impl StreamCursor for RangeCursor {
    fn next(&mut self, _ctx: &mut VmContext) -> Result<Option<Val>> {
        if self.step == 0 {
            return Err(anyhow!("range step cannot be zero"));
        }
        if let Some(end) = self.end
            && ((self.step > 0 && self.cur >= end) || (self.step < 0 && self.cur <= end))
        {
            return Ok(None);
        }
        let out = self.cur;
        self.cur += self.step;
        Ok(Some(Val::Int(out)))
    }
}

#[derive(Debug)]
struct RepeatCursor {
    value: Val,
}
impl StreamCursor for RepeatCursor {
    fn next(&mut self, _ctx: &mut VmContext) -> Result<Option<Val>> {
        Ok(Some(self.value.clone()))
    }
}

#[derive(Debug)]
struct IterateCursor {
    cur: Val,
    func: Val,
    first: bool,
}
impl StreamCursor for IterateCursor {
    fn next(&mut self, _ctx: &mut VmContext) -> Result<Option<Val>> {
        if self.first {
            self.first = false;
            return Ok(Some(self.cur.clone()));
        }
        // cur = func(cur)
        let next_val = self.func.call(std::slice::from_ref(&self.cur), _ctx)?;
        self.cur = next_val.clone();
        Ok(Some(next_val))
    }
}

struct MapCursor {
    upstream: Box<dyn StreamCursor + Send>,
    func: Val,
}
impl StreamCursor for MapCursor {
    fn next(&mut self, ctx: &mut VmContext) -> Result<Option<Val>> {
        match self.upstream.next(ctx)? {
            Some(v) => {
                let mapped = self.func.call(&[v], ctx)?;
                Ok(Some(mapped))
            }
            None => Ok(None),
        }
    }
}

struct FilterCursor {
    upstream: Box<dyn StreamCursor + Send>,
    func: Val,
}
impl StreamCursor for FilterCursor {
    fn next(&mut self, ctx: &mut VmContext) -> Result<Option<Val>> {
        loop {
            match self.upstream.next(ctx)? {
                Some(v) => {
                    let keep = self.func.call(std::slice::from_ref(&v), ctx)?;
                    let k = match keep {
                        Val::Bool(b) => b,
                        Val::Nil => false,
                        _ => true,
                    };
                    if k {
                        return Ok(Some(v));
                    }
                }
                None => return Ok(None),
            }
        }
    }
}

struct TakeCursor {
    upstream: Box<dyn StreamCursor + Send>,
    remaining: i64,
}
impl StreamCursor for TakeCursor {
    fn next(&mut self, ctx: &mut VmContext) -> Result<Option<Val>> {
        if self.remaining <= 0 {
            return Ok(None);
        }
        match self.upstream.next(ctx)? {
            Some(v) => {
                self.remaining -= 1;
                Ok(Some(v))
            }
            None => Ok(None),
        }
    }
}

struct SkipCursor {
    upstream: Box<dyn StreamCursor + Send>,
    to_skip: i64,
}
impl StreamCursor for SkipCursor {
    fn next(&mut self, ctx: &mut VmContext) -> Result<Option<Val>> {
        while self.to_skip > 0 {
            match self.upstream.next(ctx)? {
                Some(_) => self.to_skip -= 1,
                None => return Ok(None),
            }
        }
        self.upstream.next(ctx)
    }
}

struct ChainCursor {
    left: Box<dyn StreamCursor + Send>,
    right: Box<dyn StreamCursor + Send>,
    left_exhausted: bool,
}
impl StreamCursor for ChainCursor {
    fn next(&mut self, ctx: &mut VmContext) -> Result<Option<Val>> {
        if !self.left_exhausted {
            match self.left.next(ctx)? {
                Some(v) => return Ok(Some(v)),
                None => self.left_exhausted = true,
            }
        }
        self.right.next(ctx)
    }
}

struct ChannelCursor {
    channel_id: u64,
}

impl std::fmt::Debug for ChannelCursor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChannelCursor")
            .field("channel_id", &self.channel_id)
            .finish()
    }
}

impl StreamCursor for ChannelCursor {
    fn next(&mut self, _ctx: &mut VmContext) -> Result<Option<Val>> {
        let res = rt::with_runtime(|runtime| runtime.try_recv(self.channel_id))?;
        match res {
            Some((ok, value)) => {
                if ok {
                    Ok(Some(value))
                } else {
                    Ok(None)
                }
            }
            None => Ok(None),
        }
    }
}

impl StreamModule {
    pub fn new() -> Self {
        let mut functions = HashMap::new();

        // Constructors
        functions.insert("from_list".to_string(), Val::RustFunction(Self::from_list));
        functions.insert("range".to_string(), Val::RustFunction(Self::range));
        functions.insert("iterate".to_string(), Val::RustFunction(Self::iterate));
        functions.insert("repeat".to_string(), Val::RustFunction(Self::repeat));

        functions.insert("from_channel".to_string(), Val::RustFunction(Self::from_channel));

        // Transformers
        functions.insert("map".to_string(), Val::RustFunction(Self::map));
        functions.insert("filter".to_string(), Val::RustFunction(Self::filter));
        functions.insert("take".to_string(), Val::RustFunction(Self::take));
        functions.insert("skip".to_string(), Val::RustFunction(Self::skip));
        functions.insert("chain".to_string(), Val::RustFunction(Self::chain));

        // Cursors
        functions.insert("subscribe".to_string(), Val::RustFunction(Self::subscribe));
        functions.insert("next".to_string(), Val::RustFunction(Self::next));
        functions.insert("collect".to_string(), Val::RustFunction(Self::collect));

        functions.insert("next_block".to_string(), Val::RustFunction(Self::next_block));
        functions.insert("collect_block".to_string(), Val::RustFunction(Self::collect_block));

        // Register meta-methods
        register_method("List", "to_stream", Self::to_stream);

        register_method("Channel", "to_stream", Self::from_channel);

        register_method("Stream", "map", Self::map);
        register_method("Stream", "filter", Self::filter);
        register_method("Stream", "take", Self::take);
        register_method("Stream", "skip", Self::skip);
        register_method("Stream", "chain", Self::chain);
        register_method("Stream", "subscribe", Self::subscribe);
        register_method("Stream", "collect", Self::collect_stream);

        register_method("StreamCursor", "next", Self::next);
        register_method("StreamCursor", "collect", Self::collect_cursor);

        register_method("StreamCursor", "next_block", Self::next_block);
        register_method("Stream", "collect_block", Self::collect_block);
        register_method("StreamCursor", "collect_block", Self::collect_block);

        Self { functions }
    }

    fn create_stream(spec: Arc<StreamSpec>, inner: Type) -> Val {
        let id = NEXT_STREAM_ID.fetch_add(1, Ordering::Relaxed);
        STREAMS.insert(id, spec);
        Val::Stream(Arc::new(StreamValue { id, inner_type: inner }))
    }

    fn create_cursor(stream_id: u64) -> Result<Val> {
        let spec = STREAMS
            .get(&stream_id)
            .map(|entry| entry.value().clone())
            .ok_or_else(|| anyhow!("Stream not found: {}", stream_id))?;
        let cursor = spec.open_cursor();
        let id = NEXT_CURSOR_ID.fetch_add(1, Ordering::Relaxed);
        let wrapped = Arc::new(Mutex::new(cursor));
        CURSORS.insert(id, wrapped);
        // Record cursor info for blocking operations
        let mut ci = CursorInfo::default();
        if let StreamSpec::FromChannel { channel_id } = spec.as_ref() {
            ci.channel_id = Some(*channel_id);
        }
        CURSOR_INFO.insert(id, ci);
        Ok(Val::StreamCursor { id, stream_id })
    }

    // Module API implementations
    fn from_list(args: &[Val], _ctx: &mut VmContext) -> Result<Val> {
        let list = match args {
            [Val::List(l)] => l.clone(),
            _ => return Err(anyhow!("from_list expects (list)")),
        };
        let spec = Arc::new(StreamSpec::FromList(list));
        Ok(Self::create_stream(spec, Type::Any))
    }

    fn range(args: &[Val], _ctx: &mut VmContext) -> Result<Val> {
        let (start, end, step) = match args.len() {
            1 => (0, Some(extract_int(&args[0])?), 1),
            2 => (extract_int(&args[0])?, Some(extract_int(&args[1])?), 1),
            3 => (
                extract_int(&args[0])?,
                Some(extract_int(&args[1])?),
                extract_int(&args[2])?,
            ),
            _ => return Err(anyhow!("range expects 1-3 arguments")),
        };
        let spec = Arc::new(StreamSpec::Range { start, end, step });
        Ok(Self::create_stream(spec, Type::Int))
    }

    fn iterate(args: &[Val], _ctx: &mut VmContext) -> Result<Val> {
        if args.len() != 2 {
            return Err(anyhow!("iterate expects 2 arguments: seed, func"));
        }
        let seed = args[0].clone();
        let func = match &args[1] {
            Val::Closure(_) | Val::RustFunction(_) => args[1].clone(),
            other => return Err(anyhow!("iterate func must be a function, got {}", other.type_name())),
        };
        let spec = Arc::new(StreamSpec::Iterate { seed, func });
        Ok(Self::create_stream(spec, Type::Any))
    }

    fn repeat(args: &[Val], _ctx: &mut VmContext) -> Result<Val> {
        if args.len() != 1 {
            return Err(anyhow!("repeat expects 1 argument"));
        }
        let spec = Arc::new(StreamSpec::Repeat(args[0].clone()));
        Ok(Self::create_stream(spec, Type::Any))
    }

    fn from_channel(args: &[Val], _ctx: &mut VmContext) -> Result<Val> {
        let (channel_id, inner) = match args {
            [Val::Channel(channel)] => (channel.id, channel.inner_type.clone()),
            _ => return Err(anyhow!("from_channel expects (channel)")),
        };
        let spec = Arc::new(StreamSpec::FromChannel { channel_id });
        Ok(Self::create_stream(spec, inner))
    }

    fn map(args: &[Val], _ctx: &mut VmContext) -> Result<Val> {
        let (stream, func) = match args {
            [Val::Stream(stream), f] => (stream.id, f.clone()),
            _ => return Err(anyhow!("map expects (stream, func)")),
        };
        let upstream = get_stream_spec(stream)?;
        let spec = Arc::new(StreamSpec::Map { upstream, func });
        Ok(Self::create_stream(spec, Type::Any))
    }

    fn filter(args: &[Val], _ctx: &mut VmContext) -> Result<Val> {
        let (stream, func) = match args {
            [Val::Stream(stream), f] => (stream.id, f.clone()),
            _ => return Err(anyhow!("filter expects (stream, func)")),
        };
        let upstream = get_stream_spec(stream)?;
        let spec = Arc::new(StreamSpec::Filter { upstream, func });
        Ok(Self::create_stream(spec, Type::Any))
    }

    fn take(args: &[Val], _ctx: &mut VmContext) -> Result<Val> {
        let (sid, n) = match args {
            [Val::Stream(stream), n] => (stream.id, extract_int(n)?),
            _ => return Err(anyhow!("take expects (stream, n:int)")),
        };
        let upstream = get_stream_spec(sid)?;
        let spec = Arc::new(StreamSpec::Take { upstream, n });
        Ok(Self::create_stream(spec, Type::Any))
    }

    fn skip(args: &[Val], _ctx: &mut VmContext) -> Result<Val> {
        let (sid, n) = match args {
            [Val::Stream(stream), n] => (stream.id, extract_int(n)?),
            _ => return Err(anyhow!("skip expects (stream, n:int)")),
        };
        let upstream = get_stream_spec(sid)?;
        let spec = Arc::new(StreamSpec::Skip { upstream, n });
        Ok(Self::create_stream(spec, Type::Any))
    }

    fn chain(args: &[Val], _ctx: &mut VmContext) -> Result<Val> {
        let (a, b) = match args {
            [Val::Stream(a), Val::Stream(b)] => (a.id, b.id),
            _ => return Err(anyhow!("chain expects (stream, stream)")),
        };
        let left = get_stream_spec(a)?;
        let right = get_stream_spec(b)?;
        let spec = Arc::new(StreamSpec::Chain { left, right });
        Ok(Self::create_stream(spec, Type::Any))
    }

    fn subscribe(args: &[Val], _ctx: &mut VmContext) -> Result<Val> {
        let sid = match args {
            [Val::Stream(stream)] => stream.id,
            _ => return Err(anyhow!("subscribe expects (stream)")),
        };
        Self::create_cursor(sid)
    }

    fn next(args: &[Val], ctx: &mut VmContext) -> Result<Val> {
        let cid = match args {
            [Val::StreamCursor { id, .. }] => *id,
            _ => return Err(anyhow!("next expects (cursor)")),
        };
        let cursor_arc = CURSORS
            .get(&cid)
            .map(|entry| entry.value().clone())
            .ok_or_else(|| anyhow!("Cursor not found: {}", cid))?;
        let mut locked = cursor_arc.lock().map_err(|_| anyhow!("cursor mutex poisoned"))?;
        match locked.next(ctx)? {
            Some(v) => Ok(Val::List(vec![Val::Bool(true), v].into())),
            None => Ok(Val::List(vec![Val::Bool(false), Val::Nil].into())),
        }
    }

    fn collect(args: &[Val], ctx: &mut VmContext) -> Result<Val> {
        match args {
            // collect(cursor[, n])
            [Val::StreamCursor { .. }, ..] => Self::collect_cursor(args, ctx),
            // collect(stream[, n])
            [Val::Stream(_), ..] => Self::collect_stream(args, ctx),
            _ => Err(anyhow!("collect expects (stream|cursor[, n])")),
        }
    }

    fn collect_stream(args: &[Val], ctx: &mut VmContext) -> Result<Val> {
        let (stream, limit) = match args {
            [Val::Stream(stream)] => (stream.id, None),
            [Val::Stream(stream), n] => (stream.id, Some(extract_int(n)?)),
            _ => return Err(anyhow!("collect(stream[, n]) expects stream as first argument")),
        };
        let cursor_val = Self::create_cursor(stream)?;
        let mut argv: Vec<Val> = vec![cursor_val];
        if let Some(n) = limit {
            argv.push(Val::Int(n));
        }
        Self::collect_cursor(&argv, ctx)
    }

    fn collect_cursor(args: &[Val], ctx: &mut VmContext) -> Result<Val> {
        let (cid, limit) = match args {
            [Val::StreamCursor { id, .. }] => (*id, None),
            [Val::StreamCursor { id, .. }, n] => (*id, Some(extract_int(n)?)),
            _ => return Err(anyhow!("collect(cursor[, n]) expects cursor as first argument")),
        };
        let cursor_arc = CURSORS
            .get(&cid)
            .map(|entry| entry.value().clone())
            .ok_or_else(|| anyhow!("Cursor not found: {}", cid))?;
        let mut out: Vec<Val> = Vec::new();
        let mut taken: i64 = 0;
        loop {
            if let Some(max) = limit
                && taken >= max
            {
                break;
            }
            let next_opt = {
                let mut locked = cursor_arc.lock().map_err(|_| anyhow!("cursor mutex poisoned"))?;
                locked.next(ctx)?
            };
            match next_opt {
                Some(v) => {
                    out.push(v);
                    taken += 1;
                }
                None => break,
            }
        }
        Ok(Val::List(out.into()))
    }

    fn next_block(args: &[Val], ctx: &mut VmContext) -> Result<Val> {
        use std::time::Duration;
        let (cid, timeout_ms) = match args {
            [Val::StreamCursor { id, .. }] => (*id, None),
            [Val::StreamCursor { id, .. }, Val::Int(ms)] => (*id, Some(*ms)),
            _ => {
                return Err(anyhow!(
                    "next_block(cursor[, timeout_ms]) expects cursor as first argument"
                ));
            }
        };
        let info = CURSOR_INFO
            .get(&cid)
            .map(|entry| entry.value().clone())
            .unwrap_or_default();
        if let Some(ch_id) = info.channel_id {
            let res = rt::with_runtime(|rt| match timeout_ms {
                Some(ms) if ms > 0 => {
                    let fut = rt.recv_async(ch_id);
                    let res =
                        rt.block_on(async move { tokio::time::timeout(Duration::from_millis(ms as u64), fut).await });
                    match res {
                        Ok(Ok((ok, val))) => Ok(Val::List(vec![Val::Bool(ok), val].into())),
                        Ok(Err(e)) => Err(e),
                        Err(_elapsed) => Ok(Val::List(vec![Val::Bool(false), Val::Nil].into())),
                    }
                }
                _ => {
                    let (ok, val) = rt.block_on(rt.recv_async(ch_id))?;
                    Ok(Val::List(vec![Val::Bool(ok), val].into()))
                }
            })?;
            Ok(res)
        } else {
            // Fallback for non-channel cursors
            Self::next(args, ctx)
        }
    }

    fn collect_block(args: &[Val], ctx: &mut VmContext) -> Result<Val> {
        // Forms:
        // - collect_block(stream)
        // - collect_block(stream, n)
        // - collect_block(stream, n, timeout_ms)
        // - collect_block(cursor)
        // - collect_block(cursor, n)
        // - collect_block(cursor, n, timeout_ms)
        let (cursor_id, need_drop_cursor, limit, timeout_ms) = match args {
            [Val::Stream(stream)] => {
                let c = Self::create_cursor(stream.id)?;
                let Val::StreamCursor { id: cid, .. } = c else {
                    unreachable!()
                };
                (cid, Some(c), None, None)
            }
            [Val::Stream(stream), n] => {
                let c = Self::create_cursor(stream.id)?;
                let Val::StreamCursor { id: cid, .. } = c else {
                    unreachable!()
                };
                (cid, Some(c), Some(extract_int(n)?), None)
            }
            [Val::Stream(stream), n, Val::Int(ms)] => {
                let c = Self::create_cursor(stream.id)?;
                let Val::StreamCursor { id: cid, .. } = c else {
                    unreachable!()
                };
                (cid, Some(c), Some(extract_int(n)?), Some(*ms))
            }
            [Val::StreamCursor { id: cid, .. }] => (*cid, None, None, None),
            [Val::StreamCursor { id: cid, .. }, n] => (*cid, None, Some(extract_int(n)?), None),
            [Val::StreamCursor { id: cid, .. }, n, Val::Int(ms)] => (*cid, None, Some(extract_int(n)?), Some(*ms)),
            _ => return Err(anyhow!("collect_block expects (stream|cursor[, n][, timeout_ms])")),
        };

        let info = CURSOR_INFO
            .get(&cursor_id)
            .map(|entry| entry.value().clone())
            .unwrap_or_default();

        let cursor_arc = CURSORS
            .get(&cursor_id)
            .map(|entry| entry.value().clone())
            .ok_or_else(|| anyhow!("Cursor not found: {}", cursor_id))?;

        let mut out: Vec<Val> = Vec::new();
        let mut taken: i64 = 0;

        if let Some(ch_id) = info.channel_id {
            use std::time::Duration;
            rt::with_runtime(|rt| {
                loop {
                    if let Some(max) = limit
                        && taken >= max
                    {
                        break;
                    }
                    let res = match timeout_ms {
                        Some(ms) if ms > 0 => {
                            let fut = rt.recv_async(ch_id);
                            match rt.block_on(async move {
                                tokio::time::timeout(Duration::from_millis(ms as u64), fut).await
                            }) {
                                Ok(Ok((ok, val))) => Some((ok, val)),
                                Ok(Err(e)) => return Err(e),
                                Err(_elapsed) => None, // timeout for this item
                            }
                        }
                        _ => Some(rt.block_on(rt.recv_async(ch_id))?),
                    };
                    match res {
                        Some((true, v)) => {
                            out.push(v);
                            taken += 1;
                        }
                        Some((false, _)) => break, // channel closed
                        None => break,             // timeout
                    }
                }
                Ok(())
            })?;
        } else {
            // Non-channel cursor: behave like non-blocking collect
            loop {
                if let Some(max) = limit
                    && taken >= max
                {
                    break;
                }
                let next_opt = {
                    let mut locked = cursor_arc.lock().map_err(|_| anyhow!("cursor mutex poisoned"))?;
                    locked.next(ctx)?
                };
                match next_opt {
                    Some(v) => {
                        out.push(v);
                        taken += 1;
                    }
                    None => break,
                }
            }
        }

        // need_drop_cursor is not used beyond lifetime tracking; cursor will drop at end of scope
        let _ = need_drop_cursor;
        Ok(Val::List(out.into()))
    }

    fn to_stream(args: &[Val], ctx: &mut VmContext) -> Result<Val> {
        match args {
            [Val::List(_)] => Self::from_list(args, ctx),
            _ => Err(anyhow!("to_stream expects list as receiver")),
        }
    }
}

fn get_stream_spec(id: u64) -> Result<Arc<StreamSpec>> {
    STREAMS
        .get(&id)
        .map(|entry| entry.value().clone())
        .ok_or_else(|| anyhow!("Stream not found: {}", id))
}

fn extract_int(val: &Val) -> Result<i64> {
    match val {
        Val::Int(i) => Ok(*i),
        _ => Err(anyhow!("Expected integer, got {:?}", val)),
    }
}

impl Module for StreamModule {
    fn name(&self) -> &str {
        "stream"
    }
    fn description(&self) -> &str {
        "Lazy, cold stream utilities (multi-consumer)"
    }
    fn register(&self, _registry: &mut module::ModuleRegistry) -> Result<()> {
        Ok(())
    }
    fn exports(&self) -> HashMap<String, Val> {
        self.functions.clone()
    }
}
