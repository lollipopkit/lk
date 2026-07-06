//! Runtime scheduler and task management system
//!
//! Provides Go-style concurrency primitives using tokio for multithreading support.

use anyhow::{Result, anyhow};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::{Mutex as TokioMutex, mpsc};
use tokio::task::JoinHandle;

use crate::val::{HeapStore, RuntimeVal};

#[derive(Clone, Debug)]
pub struct RuntimePayload {
    pub value: RuntimeVal,
    pub heap: HeapStore,
}

impl RuntimePayload {
    pub fn new(value: RuntimeVal, heap: HeapStore) -> Self {
        Self { value, heap }
    }

    pub fn copy_from_value(value: &RuntimeVal, heap: &HeapStore) -> Result<Self> {
        let mut payload_heap = HeapStore::new();
        let value = crate::vm::copy_runtime_value(value, heap, &mut payload_heap)?;
        Ok(Self::new(value, payload_heap))
    }

    pub fn into_value(self, heap: &mut HeapStore) -> Result<RuntimeVal> {
        crate::vm::copy_runtime_value(&self.value, &self.heap, heap)
    }

    pub fn clone_value_into(&self, heap: &mut HeapStore) -> Result<RuntimeVal> {
        crate::vm::copy_runtime_value(&self.value, &self.heap, heap)
    }

    pub fn nil() -> Self {
        Self {
            value: RuntimeVal::Nil,
            heap: HeapStore::new(),
        }
    }
}

/// Task handle for spawned concurrent tasks
#[derive(Debug)]
pub struct Task {
    pub id: u64,
    pub handle: JoinHandle<Result<RuntimePayload>>,
    pub result: Option<Result<RuntimePayload>>,
}

/// Channel sender wrapper to handle both bounded and unbounded channels
#[derive(Debug)]
pub enum ChannelSender {
    Bounded(mpsc::Sender<RuntimePayload>),
    Unbounded(mpsc::UnboundedSender<RuntimePayload>),
}

impl ChannelSender {
    fn clone_sender(&self) -> Self {
        match self {
            ChannelSender::Bounded(sender) => ChannelSender::Bounded(sender.clone()),
            ChannelSender::Unbounded(sender) => ChannelSender::Unbounded(sender.clone()),
        }
    }
}

/// Channel receiver wrapper to handle both bounded and unbounded channels
#[derive(Debug)]
pub enum ChannelReceiver {
    Bounded(mpsc::Receiver<RuntimePayload>),
    Unbounded(mpsc::UnboundedReceiver<RuntimePayload>),
}

/// Channel for inter-task communication
#[derive(Debug)]
pub struct Channel {
    pub id: u64,
    pub capacity: Option<usize>,
    pub sender: ChannelSender,
    pub receiver: Arc<TokioMutex<ChannelReceiver>>,
    pub closed: Arc<AtomicBool>,
}

/// Runtime scheduler managing concurrent tasks and channels
#[derive(Debug)]
pub struct Runtime {
    tasks: Arc<Mutex<HashMap<u64, Task>>>,
    channels: Arc<Mutex<HashMap<u64, Channel>>>,
    next_task_id: Arc<Mutex<u64>>,
    next_channel_id: Arc<Mutex<u64>>,
    tokio_runtime: tokio::runtime::Runtime,
}

impl Runtime {
    /// Create a new multi-threaded runtime
    pub fn new_multi_thread() -> Result<Self> {
        let tokio_runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .map_err(|e| anyhow!("Failed to create tokio runtime: {}", e))?;

        Ok(Runtime {
            tasks: Arc::new(Mutex::new(HashMap::new())),
            channels: Arc::new(Mutex::new(HashMap::new())),
            next_task_id: Arc::new(Mutex::new(1)),
            next_channel_id: Arc::new(Mutex::new(1)),
            tokio_runtime,
        })
    }

    /// Create a new current-thread runtime for testing
    pub fn new_current_thread() -> Result<Self> {
        let tokio_runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| anyhow!("Failed to create tokio runtime: {}", e))?;

        Ok(Runtime {
            tasks: Arc::new(Mutex::new(HashMap::new())),
            channels: Arc::new(Mutex::new(HashMap::new())),
            next_task_id: Arc::new(Mutex::new(1)),
            next_channel_id: Arc::new(Mutex::new(1)),
            tokio_runtime,
        })
    }

    /// Spawn a new task
    pub fn spawn<F>(&self, future: F) -> Result<u64>
    where
        F: std::future::Future<Output = Result<RuntimePayload>> + Send + 'static,
    {
        let task_id = {
            let mut next_id = self.next_task_id.lock().unwrap();
            let id = *next_id;
            *next_id += 1;
            id
        };

        let handle = self.tokio_runtime.spawn(future);

        let task = Task {
            id: task_id,
            handle,
            result: None,
        };

        let mut tasks = self.tasks.lock().unwrap();
        tasks.insert(task_id, task);

        Ok(task_id)
    }

    /// Create a new channel
    pub fn create_channel(&self, capacity: Option<usize>) -> Result<u64> {
        let channel_id = {
            let mut next_id = self.next_channel_id.lock().unwrap();
            let id = *next_id;
            *next_id += 1;
            id
        };

        let channel = if let Some(cap) = capacity {
            let (sender, receiver) = mpsc::channel::<RuntimePayload>(cap);
            Channel {
                id: channel_id,
                capacity: Some(cap),
                sender: ChannelSender::Bounded(sender),
                receiver: Arc::new(TokioMutex::new(ChannelReceiver::Bounded(receiver))),
                closed: Arc::new(AtomicBool::new(false)),
            }
        } else {
            let (sender, receiver) = mpsc::unbounded_channel::<RuntimePayload>();
            Channel {
                id: channel_id,
                capacity: None,
                sender: ChannelSender::Unbounded(sender),
                receiver: Arc::new(TokioMutex::new(ChannelReceiver::Unbounded(receiver))),
                closed: Arc::new(AtomicBool::new(false)),
            }
        };

        let mut channels = self.channels.lock().unwrap();
        channels.insert(channel_id, channel);

        Ok(channel_id)
    }

    /// Attempt to send a value without blocking.
    pub fn try_send(&self, channel_id: u64, value: RuntimePayload) -> Result<bool> {
        let (sender, closed_flag) = {
            let channels = self.channels.lock().unwrap();
            let channel = channels.get(&channel_id).ok_or_else(|| anyhow!("Channel not found"))?;
            (channel.sender.clone_sender(), channel.closed.clone())
        };

        match sender {
            ChannelSender::Bounded(sender) => match sender.try_send(value) {
                Ok(()) => Ok(true),
                Err(mpsc::error::TrySendError::Full(_)) => Ok(false),
                Err(mpsc::error::TrySendError::Closed(_)) => {
                    closed_flag.store(true, Ordering::SeqCst);
                    Err(anyhow!("Channel is closed"))
                }
            },
            ChannelSender::Unbounded(sender) => match sender.send(value) {
                Ok(()) => Ok(true),
                Err(_) => {
                    closed_flag.store(true, Ordering::SeqCst);
                    Err(anyhow!("Channel is closed"))
                }
            },
        }
    }

    /// Send a value to a channel, awaiting until it can be delivered or the channel closes.
    pub async fn send_async(&self, channel_id: u64, value: RuntimePayload) -> Result<bool> {
        let (sender, closed_flag) = {
            let channels = self.channels.lock().unwrap();
            let channel = channels.get(&channel_id).ok_or_else(|| anyhow!("Channel not found"))?;
            (channel.sender.clone_sender(), channel.closed.clone())
        };

        match sender {
            ChannelSender::Bounded(sender) => match sender.send(value).await {
                Ok(()) => Ok(true),
                Err(_) => {
                    closed_flag.store(true, Ordering::SeqCst);
                    Ok(false)
                }
            },
            ChannelSender::Unbounded(sender) => match sender.send(value) {
                Ok(()) => Ok(true),
                Err(_) => {
                    closed_flag.store(true, Ordering::SeqCst);
                    Ok(false)
                }
            },
        }
    }

    /// Try to receive a value without blocking. Returns None if no value is ready.
    pub fn try_recv(&self, channel_id: u64) -> Result<Option<(bool, RuntimePayload)>> {
        let (receiver_arc, closed_flag) = {
            let channels = self.channels.lock().unwrap();
            let channel = channels.get(&channel_id).ok_or_else(|| anyhow!("Channel not found"))?;
            (channel.receiver.clone(), channel.closed.clone())
        };

        let mut receiver = match receiver_arc.try_lock() {
            Ok(guard) => guard,
            Err(_) => return Ok(None),
        };

        let result = match &mut *receiver {
            ChannelReceiver::Bounded(recv) => match recv.try_recv() {
                Ok(value) => Some((true, value)),
                Err(mpsc::error::TryRecvError::Empty) => {
                    if closed_flag.load(Ordering::SeqCst) {
                        Some((false, RuntimePayload::nil()))
                    } else {
                        None
                    }
                }
                Err(mpsc::error::TryRecvError::Disconnected) => {
                    closed_flag.store(true, Ordering::SeqCst);
                    Some((false, RuntimePayload::nil()))
                }
            },
            ChannelReceiver::Unbounded(recv) => match recv.try_recv() {
                Ok(value) => Some((true, value)),
                Err(mpsc::error::TryRecvError::Empty) => {
                    if closed_flag.load(Ordering::SeqCst) {
                        Some((false, RuntimePayload::nil()))
                    } else {
                        None
                    }
                }
                Err(mpsc::error::TryRecvError::Disconnected) => {
                    closed_flag.store(true, Ordering::SeqCst);
                    Some((false, RuntimePayload::nil()))
                }
            },
        };

        Ok(result)
    }

    /// Return the number of queued values currently buffered for a channel.
    pub fn channel_len(&self, channel_id: u64) -> Result<usize> {
        let receiver_arc = {
            let channels = self.channels.lock().unwrap();
            let Some(channel) = channels.get(&channel_id) else {
                return Ok(0);
            };
            channel.receiver.clone()
        };
        let Ok(receiver) = receiver_arc.try_lock() else {
            return Ok(0);
        };
        Ok(match &*receiver {
            ChannelReceiver::Bounded(receiver) => receiver.len(),
            ChannelReceiver::Unbounded(receiver) => receiver.len(),
        })
    }

    /// Return whether a channel has been closed or removed from the runtime.
    pub fn channel_is_closed(&self, channel_id: u64) -> Result<bool> {
        let channels = self.channels.lock().unwrap();
        let Some(channel) = channels.get(&channel_id) else {
            return Ok(true);
        };
        Ok(channel.closed.load(Ordering::SeqCst))
    }

    /// Receive a value from a channel, waiting until a value is available or the channel closes.
    pub async fn recv_async(&self, channel_id: u64) -> Result<(bool, RuntimePayload)> {
        let (receiver_arc, closed_flag) = {
            let channels = self.channels.lock().unwrap();
            let channel = channels.get(&channel_id).ok_or_else(|| anyhow!("Channel not found"))?;
            (channel.receiver.clone(), channel.closed.clone())
        };

        let mut receiver = receiver_arc.lock().await;
        let value = match &mut *receiver {
            ChannelReceiver::Bounded(recv) => recv.recv().await,
            ChannelReceiver::Unbounded(recv) => recv.recv().await,
        };

        match value {
            Some(value) => Ok((true, value)),
            None => {
                closed_flag.store(true, Ordering::SeqCst);
                Ok((false, RuntimePayload::nil()))
            }
        }
    }

    /// Close a channel
    pub fn close_channel(&self, channel_id: u64) -> Result<()> {
        let mut channels = self.channels.lock().unwrap();
        if let Some(channel) = channels.remove(&channel_id) {
            channel.closed.store(true, Ordering::SeqCst);
            Ok(())
        } else {
            Err(anyhow!("Channel not found"))
        }
    }

    /// Wait for a task to complete
    pub async fn join_task(&self, task_id: u64) -> Result<RuntimePayload> {
        let mut task = {
            let mut tasks = self.tasks.lock().unwrap();
            tasks.remove(&task_id).ok_or_else(|| anyhow!("Task not found"))?
        };

        // If result is already available, return it
        if let Some(result) = task.result.take() {
            return result;
        }

        // Otherwise await the handle
        match task.handle.await {
            Ok(result) => result,
            Err(e) => Err(anyhow!("Task failed: {}", e)),
        }
    }

    /// Cancel a task
    pub fn cancel_task(&self, task_id: u64) -> Result<()> {
        let mut tasks = self.tasks.lock().unwrap();

        if let Some(task) = tasks.remove(&task_id) {
            task.handle.abort();
        } else {
            // Task may have already completed or the runtime might have been reset; treat as a no-op.
            return Ok(());
        }

        Ok(())
    }

    /// Get runtime statistics
    pub fn stats(&self) -> RuntimeStats {
        let tasks = self.tasks.lock().unwrap();
        let channels = self.channels.lock().unwrap();

        RuntimeStats {
            active_tasks: tasks.len(),
            active_channels: channels.len(),
            is_multi_threaded: matches!(
                self.tokio_runtime.handle().runtime_flavor(),
                tokio::runtime::RuntimeFlavor::MultiThread
            ),
        }
    }

    /// Block on a future using the tokio runtime
    pub fn block_on<F>(&self, future: F) -> F::Output
    where
        F: std::future::Future,
    {
        // Blocking channel ops (`send`/`recv`/`select`) are also called from
        // *inside* goroutines — i.e. from a tokio worker thread, where a
        // plain `block_on` panics ("cannot start a runtime from within a
        // runtime"). On the multi-thread flavor, `block_in_place` parks this
        // worker's other tasks elsewhere first, making the Go idiom (a
        // goroutine blocking on a channel) safe. The current-thread flavor
        // (LK_SINGLE_THREAD test fallback) has no other worker to hand off
        // to; there the old direct call remains.
        if tokio::runtime::Handle::try_current().is_ok()
            && matches!(
                self.tokio_runtime.handle().runtime_flavor(),
                tokio::runtime::RuntimeFlavor::MultiThread
            )
        {
            tokio::task::block_in_place(|| self.tokio_runtime.handle().block_on(future))
        } else {
            self.tokio_runtime.block_on(future)
        }
    }
}

/// Runtime statistics
#[derive(Debug, Clone)]
pub struct RuntimeStats {
    pub active_tasks: usize,
    pub active_channels: usize,
    pub is_multi_threaded: bool,
}

fn drop_runtime_arc(runtime: Arc<Runtime>) {
    if tokio::runtime::Handle::try_current().is_ok() {
        std::thread::spawn(move || drop(runtime));
    } else {
        drop(runtime);
    }
}

fn create_runtime() -> Result<Runtime> {
    if std::env::var("LK_SINGLE_THREAD").is_ok() {
        return Runtime::new_current_thread();
    }

    match Runtime::new_multi_thread() {
        Ok(runtime) => Ok(runtime),
        Err(err) => {
            let err_msg = err.to_string();
            Runtime::new_current_thread().map_err(|fallback_err| {
                anyhow!(
                    "Failed to create multi-thread runtime ({}) and fallback to current-thread runtime failed ({})",
                    err_msg,
                    fallback_err
                )
            })
        }
    }
}

/// Shareable handle to the async (tokio) runtime.
///
/// Replaces the former process-global `GLOBAL_RUNTIME`. The handle lives on
/// `VmContext`; clones share the same lazily-initialized runtime, so a VM and
/// any contexts derived from it (spawned tasks, shallow clones) run on one
/// reactor. Cheap to clone and `Send + Sync`, so it can be captured into
/// spawned futures.
#[derive(Clone, Default)]
pub struct AsyncRuntimeHandle {
    inner: Arc<Mutex<Option<Arc<Runtime>>>>,
}

impl std::fmt::Debug for AsyncRuntimeHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let initialized = self.inner.lock().map(|guard| guard.is_some()).unwrap_or(false);
        f.debug_struct("AsyncRuntimeHandle")
            .field("initialized", &initialized)
            .finish()
    }
}

impl AsyncRuntimeHandle {
    pub fn new() -> Self {
        Self::default()
    }

    fn get_or_init(&self) -> Result<Arc<Runtime>> {
        let mut guard = self.inner.lock().unwrap();
        if guard.is_none() {
            *guard = Some(Arc::new(create_runtime()?));
        }
        guard
            .as_ref()
            .cloned()
            .ok_or_else(|| anyhow!("Runtime initialization failed"))
    }

    /// Eagerly initialize the runtime (optional warm-up).
    pub fn init(&self) -> Result<()> {
        self.get_or_init().map(|_| ())
    }

    /// Run `f` with a reference to the runtime, initializing it on first use.
    pub fn with<F, R>(&self, f: F) -> Result<R>
    where
        F: FnOnce(&Runtime) -> Result<R>,
    {
        let runtime_arc = self.get_or_init()?;
        let result = f(runtime_arc.as_ref());
        drop_runtime_arc(runtime_arc);
        result
    }

    /// Shut down and drop the runtime if it was initialized.
    pub fn shutdown(&self) {
        let runtime_arc = {
            let mut guard = self.inner.lock().unwrap();
            guard.take()
        };
        if let Some(runtime) = runtime_arc {
            drop_runtime_arc(runtime);
        }
    }
}

/// Select operation result
#[derive(Debug, Clone)]
pub struct SelectResult {
    pub case_index: Option<usize>,
    pub recv_payload: Option<(bool, RuntimePayload)>,
    pub is_default: bool,
}

#[derive(Debug, Clone)]
pub enum SelectArm {
    Recv {
        case_index: usize,
        channel_id: u64,
    },
    Send {
        case_index: usize,
        channel_id: u64,
        value: RuntimePayload,
    },
}

/// Select operation for multiple channel operations
pub struct SelectOperation {
    arms: Vec<SelectArm>,
}

impl Default for SelectOperation {
    fn default() -> Self {
        Self::new()
    }
}

impl SelectOperation {
    pub fn new() -> Self {
        Self { arms: Vec::new() }
    }

    pub fn add_recv(&mut self, case_index: usize, channel_id: u64) {
        self.arms.push(SelectArm::Recv { case_index, channel_id });
    }

    pub fn add_send(&mut self, case_index: usize, channel_id: u64, value: RuntimePayload) {
        self.arms.push(SelectArm::Send {
            case_index,
            channel_id,
            value,
        });
    }

    pub fn is_empty(&self) -> bool {
        self.arms.is_empty()
    }

    /// Execute select operation and return the first available operation
    pub async fn execute(&self, runtime: &Runtime, has_default: bool) -> Result<SelectResult> {
        // Try fast path first to avoid awaiting when an operation is ready
        for arm in &self.arms {
            match arm {
                SelectArm::Recv { case_index, channel_id } => {
                    if let Some((ok, value)) = runtime.try_recv(*channel_id)? {
                        return Ok(SelectResult {
                            case_index: Some(*case_index),
                            recv_payload: Some((ok, value)),
                            is_default: false,
                        });
                    }
                }
                SelectArm::Send {
                    case_index,
                    channel_id,
                    value,
                } => match runtime.try_send(*channel_id, value.clone()) {
                    Ok(true) => {
                        return Ok(SelectResult {
                            case_index: Some(*case_index),
                            recv_payload: None,
                            is_default: false,
                        });
                    }
                    Ok(false) => {}
                    Err(e) => return Err(e),
                },
            }
        }

        if has_default {
            return Ok(SelectResult {
                case_index: None,
                recv_payload: None,
                is_default: true,
            });
        }

        if self.arms.is_empty() {
            return Ok(SelectResult {
                case_index: None,
                recv_payload: None,
                is_default: true,
            });
        }

        use futures::future::select_all;
        use std::pin::Pin;

        let mut futures: Vec<Pin<Box<dyn futures::Future<Output = Result<SelectResult>> + Send>>> =
            Vec::with_capacity(self.arms.len());

        for arm in &self.arms {
            match arm.clone() {
                SelectArm::Recv { case_index, channel_id } => {
                    let fut = async move {
                        let (ok, value) = runtime.recv_async(channel_id).await?;
                        Ok(SelectResult {
                            case_index: Some(case_index),
                            recv_payload: Some((ok, value)),
                            is_default: false,
                        })
                    };
                    futures.push(Box::pin(fut));
                }
                SelectArm::Send {
                    case_index,
                    channel_id,
                    value,
                } => {
                    let fut = async move {
                        let sent = runtime.send_async(channel_id, value).await?;
                        if sent {
                            Ok(SelectResult {
                                case_index: Some(case_index),
                                recv_payload: None,
                                is_default: false,
                            })
                        } else {
                            Err(anyhow!("Channel closed during send"))
                        }
                    };
                    futures.push(Box::pin(fut));
                }
            }
        }

        let (result, _index, _remaining) = select_all(futures).await;
        result
    }
}
