use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use serde::Serialize;

use super::{Bc32Function, Function, PackIssue};

#[derive(Default)]
struct Bc32Metrics {
    attempts: u64,
    packed: u64,
    total_ops: u64,
    total_words: u64,
    fallback_by_reason: HashMap<&'static str, u64>,
    fallback_by_opcode: HashMap<&'static str, u64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Bc32ReasonEntry {
    pub reason: String,
    pub count: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Bc32OpcodeEntry {
    pub opcode: String,
    pub count: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Bc32MetricsSnapshot {
    pub attempts: u64,
    pub packed: u64,
    pub total_ops: u64,
    pub total_words: u64,
    pub fallback_reasons: Vec<Bc32ReasonEntry>,
    pub fallback_opcodes: Vec<Bc32OpcodeEntry>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Bc32PackStatus {
    pub packed: bool,
    pub ops: usize,
    pub words: Option<usize>,
    pub reason: Option<String>,
    pub opcode: Option<String>,
    pub detail: Option<String>,
    pub op_index: Option<usize>,
}

impl Bc32PackStatus {
    fn packed(ops: usize, words: usize) -> Self {
        Self {
            packed: true,
            ops,
            words: Some(words),
            reason: None,
            opcode: None,
            detail: None,
            op_index: None,
        }
    }

    fn fallback(ops: usize, issue: PackIssue) -> Self {
        let PackIssue { reason, op_index } = issue;
        Self {
            packed: false,
            ops,
            words: None,
            reason: Some(reason.reason_key().to_string()),
            opcode: Some(reason.opcode().to_string()),
            detail: Some(reason.detail().to_string()),
            op_index,
        }
    }
}

static METRICS: OnceLock<Mutex<Bc32Metrics>> = OnceLock::new();

fn metrics() -> &'static Mutex<Bc32Metrics> {
    METRICS.get_or_init(|| Mutex::new(Bc32Metrics::default()))
}

pub(super) fn record_attempt(ops: usize) {
    let mut guard = metrics().lock().expect("bc32 metrics poisoned");
    guard.attempts += 1;
    guard.total_ops += ops as u64;
}

pub(super) fn record_success(words: usize) {
    let mut guard = metrics().lock().expect("bc32 metrics poisoned");
    guard.packed += 1;
    guard.total_words += words as u64;
}

pub(super) fn record_failure(reason: &'static str, opcode: &'static str) {
    let mut guard = metrics().lock().expect("bc32 metrics poisoned");
    *guard.fallback_by_reason.entry(reason).or_default() += 1;
    *guard.fallback_by_opcode.entry(opcode).or_default() += 1;
}

pub fn bc32_metrics_snapshot() -> Bc32MetricsSnapshot {
    let guard = metrics().lock().expect("bc32 metrics poisoned");
    let mut fallback_reasons: Vec<Bc32ReasonEntry> = guard
        .fallback_by_reason
        .iter()
        .map(|(reason, count)| Bc32ReasonEntry {
            reason: (*reason).to_string(),
            count: *count,
        })
        .collect();
    fallback_reasons.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.reason.cmp(&b.reason)));

    let mut fallback_opcodes: Vec<Bc32OpcodeEntry> = guard
        .fallback_by_opcode
        .iter()
        .map(|(opcode, count)| Bc32OpcodeEntry {
            opcode: (*opcode).to_string(),
            count: *count,
        })
        .collect();
    fallback_opcodes.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.opcode.cmp(&b.opcode)));

    Bc32MetricsSnapshot {
        attempts: guard.attempts,
        packed: guard.packed,
        total_ops: guard.total_ops,
        total_words: guard.total_words,
        fallback_reasons,
        fallback_opcodes,
    }
}

pub fn bc32_metrics_reset() {
    let mut guard = metrics().lock().expect("bc32 metrics poisoned");
    *guard = Bc32Metrics::default();
}

pub fn bc32_pack_status(function: &Function) -> Bc32PackStatus {
    match Bc32Function::try_pack(function) {
        Ok(packed) => Bc32PackStatus::packed(function.code.len(), packed.code32.len()),
        Err(issue) => Bc32PackStatus::fallback(function.code.len(), issue),
    }
}
