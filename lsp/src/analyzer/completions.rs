use super::LkrAnalyzer;
use lkr_core::{expr::Expr, val::Val};
use tower_lsp::lsp_types::{CompletionItem, CompletionItemKind, Diagnostic, DiagnosticSeverity, Position, Range};

impl LkrAnalyzer {
    /// Get common variable completions for the given prefix
    pub fn get_var_completions(&mut self, prefix: &str) -> Vec<CompletionItem> {
        // Use cached completion items if available
        let all_items = if let Some(ref cached) = self.completion_cache {
            cached.clone()
        } else {
            let mut items = Vec::new();

            // Common variable patterns (without legacy '@')
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

                if let Ok(m) = self.registry.get_module(&module_name) {
                    let exports = m.exports();
                    for (k, v) in exports {
                        let label = format!("{}.{}", module_name, k);
                        let (kind, detail) = match v {
                            Val::RustFunction(_) | Val::RustFunctionNamed(_) | Val::Closure(_) => {
                                (CompletionItemKind::FUNCTION, "function".to_string())
                            }
                            Val::Int(_) | Val::Float(_) | Val::Bool(_) | Val::Str(_) => {
                                (CompletionItemKind::CONSTANT, "const".to_string())
                            }
                            Val::List(_) => (CompletionItemKind::VARIABLE, "list".to_string()),
                            Val::Map(_) => (CompletionItemKind::MODULE, "namespace".to_string()),
                            Val::Task(_) => (CompletionItemKind::VALUE, "task".to_string()),
                            Val::Channel(_) => (CompletionItemKind::VALUE, "channel".to_string()),
                            Val::Stream(_) => (CompletionItemKind::VALUE, "stream".to_string()),
                            Val::Iterator(_) => (CompletionItemKind::VALUE, "iterator".to_string()),
                            Val::MutationGuard(_) => (CompletionItemKind::VALUE, "mutation guard".to_string()),
                            Val::StreamCursor { .. } => (CompletionItemKind::VALUE, "stream cursor".to_string()),
                            Val::Object(_) => (CompletionItemKind::VALUE, "object".to_string()),
                            Val::Nil => (CompletionItemKind::VALUE, "nil".to_string()),
                        };
                        items.push(CompletionItem {
                            label,
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
    pub fn validate_identifier_access(&self, expr: &Expr, context: Option<&Val>) -> Vec<Diagnostic> {
        let mut diagnostics = Vec::new();

        let required_ctx = expr.requested_ctx();

        if let Some(ctx) = context {
            // Check if required identifier roots are available
            for ctx_key in &required_ctx {
                if !self.vars_has_key(ctx, ctx_key) {
                    diagnostics.push(Diagnostic::new(
                        Range::new(Position::new(0, 0), Position::new(0, 100)),
                        Some(DiagnosticSeverity::WARNING),
                        None,
                        Some("lkr".to_string()),
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
                Some("lkr".to_string()),
                format!("Expression references identifier roots: {:?}", required_ctx),
                None,
                None,
            ));
        }

        diagnostics
    }

    pub(crate) fn vars_has_key(&self, context: &Val, key: &str) -> bool {
        // Simple key existence check - traverse dot notation
        let parts: Vec<&str> = key.split('.').collect();
        let mut current = context;

        for part in parts {
            match current {
                Val::Map(map) => {
                    if let Some(value) = map.get(part) {
                        current = value;
                    } else {
                        return false;
                    }
                }
                _ => return false,
            }
        }
        true
    }
}
