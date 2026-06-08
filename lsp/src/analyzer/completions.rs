use super::LkAnalyzer;
use lk_core::expr::Expr;
use lk_core::val::{HeapStore, HeapValue, RuntimeVal, TypedMap};
use std::collections::BTreeMap;
use tower_lsp::lsp_types::{Diagnostic, DiagnosticSeverity, Position, Range};

impl LkAnalyzer {
    /// Validate identifier access in an expression against an optional variables map.
    pub fn validate_identifier_access(
        &self,
        expr: &Expr,
        context: Option<(&RuntimeVal, &HeapStore)>,
    ) -> Vec<Diagnostic> {
        let mut diagnostics = Vec::new();
        let required_ctx = expr.requested_ctx();

        if let Some((ctx, heap)) = context {
            for ctx_key in &required_ctx {
                if !self.vars_has_key(ctx, heap, ctx_key) {
                    diagnostics.push(Diagnostic::new(
                        Range::new(Position::new(0, 0), Position::new(0, 100)),
                        Some(DiagnosticSeverity::WARNING),
                        None,
                        Some("lk".to_string()),
                        format!("Identifier root '{}' not found in provided variables", ctx_key),
                        None,
                        None,
                    ));
                }
            }
        } else if !required_ctx.is_empty() {
            diagnostics.push(Diagnostic::new(
                Range::new(Position::new(0, 0), Position::new(0, 100)),
                Some(DiagnosticSeverity::INFORMATION),
                None,
                Some("lk".to_string()),
                format!("Expression references identifier roots: {:?}", required_ctx),
                None,
                None,
            ));
        }

        diagnostics
    }

    pub(crate) fn vars_has_key(&self, context: &RuntimeVal, heap: &HeapStore, key: &str) -> bool {
        let mut current = context.clone();
        for part in key.split('.') {
            let Some(value) = runtime_map_get_str(&current, heap, part) else {
                return false;
            };
            current = value;
        }
        true
    }

    pub(crate) fn module_export_names(&self, module_name: &str) -> Option<Vec<String>> {
        let export = self.registry.get_module(module_name).ok()?.runtime_exports().ok()?;
        let state = export.state_lock().ok()?;
        let entries = runtime_string_map_entries(export.value(), state.heap())?;
        let mut keys: Vec<String> = entries.keys().cloned().collect();
        keys.sort();
        Some(keys)
    }
}

fn runtime_map_get_str(value: &RuntimeVal, heap: &HeapStore, key: &str) -> Option<RuntimeVal> {
    let RuntimeVal::Obj(handle) = value else {
        return None;
    };
    let HeapValue::Map(map) = heap.get(*handle)? else {
        return None;
    };
    typed_map_get_str(map, key)
}

fn runtime_string_map_entries(value: &RuntimeVal, heap: &HeapStore) -> Option<BTreeMap<String, RuntimeVal>> {
    let RuntimeVal::Obj(handle) = value else {
        return None;
    };
    let HeapValue::Map(map) = heap.get(*handle)? else {
        return None;
    };
    Some(typed_map_string_entries(map))
}

fn typed_map_get_str(map: &TypedMap, key: &str) -> Option<RuntimeVal> {
    match map {
        TypedMap::Mixed(entries) => entries
            .iter()
            .find_map(|(entry_key, value)| (entry_key.as_str() == Some(key)).then(|| value.clone())),
        TypedMap::StringMixed(entries) => entries.get(key).cloned(),
        TypedMap::StringInt(entries) => entries.get(key).copied().map(RuntimeVal::Int),
        TypedMap::StringFloat(entries) => entries.get(key).copied().map(RuntimeVal::Float),
        TypedMap::StringBool(entries) => entries.get(key).copied().map(RuntimeVal::Bool),
    }
}

fn typed_map_string_entries(map: &TypedMap) -> BTreeMap<String, RuntimeVal> {
    match map {
        TypedMap::Mixed(entries) => entries
            .iter()
            .filter_map(|(key, value)| key.as_str().map(|key| (key.to_string(), value.clone())))
            .collect(),
        TypedMap::StringMixed(entries) => entries
            .iter()
            .map(|(key, value)| (key.to_string(), value.clone()))
            .collect(),
        TypedMap::StringInt(entries) => entries
            .iter()
            .map(|(key, value)| (key.to_string(), RuntimeVal::Int(*value)))
            .collect(),
        TypedMap::StringFloat(entries) => entries
            .iter()
            .map(|(key, value)| (key.to_string(), RuntimeVal::Float(*value)))
            .collect(),
        TypedMap::StringBool(entries) => entries
            .iter()
            .map(|(key, value)| (key.to_string(), RuntimeVal::Bool(*value)))
            .collect(),
    }
}
