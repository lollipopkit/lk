use super::LkAnalyzer;
use lk_core::expr::Expr;
use lk_core::val::{HeapStore, HeapValue, RuntimeVal, TypedMap};
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
        Some(lk_stdlib::stdlib_catalog().module(module_name)?.export_names())
    }
}

fn runtime_map_get_str(value: &RuntimeVal, heap: &HeapStore, key: &str) -> Option<RuntimeVal> {
    let RuntimeVal::Obj(handle) = value else {
        return None;
    };
    let HeapValue::Map(map) = heap.get(*handle)? else {
        return None;
    };
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
