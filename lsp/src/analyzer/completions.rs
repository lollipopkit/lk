use super::LkAnalyzer;
use lk_core::{
    expr::Expr,
    val::{HeapStore, HeapValue, RuntimeVal, TypedMap},
};
use std::collections::BTreeMap;
use tower_lsp::lsp_types::{CompletionItem, CompletionItemKind, Diagnostic, DiagnosticSeverity, Position, Range};

impl LkAnalyzer {
    /// Get common variable completions for the given prefix
    pub fn get_var_completions(&mut self, prefix: &str) -> Vec<CompletionItem> {
        // Use cached completion items if available
        let all_items = if let Some(ref cached) = self.completion_cache {
            cached.clone()
        } else {
            let mut items = Vec::new();

            // Common variable patterns.
            let common_contexts = [
                ("req", "Request object"),
                ("req.user", "User information"),
                ("req.user.id", "User ID"),
                ("req.user.role", "User role"),
                ("req.user.name", "User name"),
                ("record", "Record object"),
                ("record.id", "Record ID"),
                ("record.owner", "Record owner"),
                ("record.granted", "Granted users list"),
                ("env", "Environment variables"),
                ("time", "Current timestamp"),
            ];

            for (context, desc) in common_contexts {
                items.push(CompletionItem {
                    label: context.to_string(),
                    kind: Some(CompletionItemKind::PROPERTY),
                    detail: Some(desc.to_string()),
                    ..Default::default()
                });
            }

            // Stdlib modules and their exports, e.g., "iter.zip"
            for module_name in self.registry.get_module_names() {
                // module entry itself
                items.push(CompletionItem {
                    label: module_name.clone(),
                    kind: Some(CompletionItemKind::MODULE),
                    detail: Some("stdlib module".to_string()),
                    ..Default::default()
                });

                if let Some(exports) = self.module_export_completions(&module_name) {
                    for (k, kind, detail) in exports {
                        items.push(CompletionItem {
                            label: format!("{}.{}", module_name, k),
                            kind: Some(kind),
                            detail: Some(format!("{}.{}: {}", module_name, k, detail)),
                            ..Default::default()
                        });
                    }
                }
            }

            // Cache the items for future use
            self.completion_cache = Some(items.clone());
            items
        };

        // Filter by prefix
        all_items
            .into_iter()
            .filter(|item| item.label.starts_with(prefix))
            .collect()
    }

    /// Validate identifier access in an expression against an optional variables map
    pub fn validate_identifier_access(
        &self,
        expr: &Expr,
        context: Option<(&RuntimeVal, &HeapStore)>,
    ) -> Vec<Diagnostic> {
        let mut diagnostics = Vec::new();

        let required_ctx = expr.requested_ctx();

        if let Some((ctx, heap)) = context {
            // Check if required identifier roots are available
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
        // Simple key existence check - traverse dot notation
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

    fn module_export_completions(&self, module_name: &str) -> Option<Vec<(String, CompletionItemKind, String)>> {
        let export = self.registry.get_module(module_name).ok()?.runtime_exports().ok()?;
        let state = export.state_lock().ok()?;
        let entries = runtime_string_map_entries(export.value(), state.heap())?;
        let mut items: Vec<_> = entries
            .into_iter()
            .map(|(name, value)| {
                let (kind, detail) = runtime_completion_kind(&value, state.heap());
                (name, kind, detail)
            })
            .collect();
        items.sort_by(|left, right| left.0.cmp(&right.0));
        Some(items)
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

fn runtime_completion_kind(value: &RuntimeVal, heap: &HeapStore) -> (CompletionItemKind, String) {
    match value {
        RuntimeVal::Nil => (CompletionItemKind::VALUE, "Nil".to_string()),
        RuntimeVal::Bool(_) | RuntimeVal::Int(_) | RuntimeVal::Float(_) | RuntimeVal::ShortStr(_) => {
            (CompletionItemKind::CONSTANT, "const".to_string())
        }
        RuntimeVal::Obj(handle) => match heap.get(*handle) {
            Some(HeapValue::Callable(_)) => (CompletionItemKind::FUNCTION, "function".to_string()),
            Some(HeapValue::List(_)) => (CompletionItemKind::VARIABLE, "list".to_string()),
            Some(HeapValue::Map(_)) => (CompletionItemKind::MODULE, "namespace".to_string()),
            Some(HeapValue::String(_)) => (CompletionItemKind::CONSTANT, "const".to_string()),
            Some(other) => (CompletionItemKind::VALUE, other.type_name().to_string()),
            None => (CompletionItemKind::VALUE, "dangling heap ref".to_string()),
        },
    }
}
