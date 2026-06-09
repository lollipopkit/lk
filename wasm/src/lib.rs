use std::sync::Arc;

use lk_core::{
    module::ModuleRegistry,
    stmt::{ModuleResolver, StmtParser},
    token::Tokenizer,
    typ::TypeChecker,
    vm::{VmContext, execute_program_with_ctx_and_budget},
};
use lk_stdlib_common::runtime_native::runtime_display_value;
use serde::Serialize;
use wasm_bindgen::prelude::*;

const DEFAULT_INSTRUCTION_BUDGET: u64 = 1_000_000;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RunResult {
    ok: bool,
    stdout: String,
    result: Option<String>,
    error: Option<String>,
    diagnostics: Vec<Diagnostic>,
    elapsed_ms: f64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Diagnostic {
    level: &'static str,
    message: String,
    rendered: Option<String>,
    line: Option<u32>,
    column: Option<u32>,
}

#[wasm_bindgen(js_name = runLk)]
pub fn run_lk(source: &str) -> JsValue {
    let started = js_sys::Date::now();
    lk_stdlib_web::clear_stdout();
    let result = run_lk_inner(source);
    let elapsed_ms = js_sys::Date::now() - started;

    let output = match result {
        Ok(result) => RunResult {
            ok: true,
            stdout: lk_stdlib_web::take_stdout(),
            result: Some(result),
            error: None,
            diagnostics: Vec::new(),
            elapsed_ms,
        },
        Err(error) => RunResult {
            ok: false,
            stdout: lk_stdlib_web::take_stdout(),
            result: None,
            error: Some(error.message.clone()),
            diagnostics: vec![error],
            elapsed_ms,
        },
    };

    serde_wasm_bindgen::to_value(&output).expect("RunResult should serialize")
}

fn run_lk_inner(source: &str) -> Result<String, Diagnostic> {
    let (tokens, spans) = Tokenizer::tokenize_enhanced_with_spans(source).map_err(|error| Diagnostic {
        level: "error",
        message: error.to_string(),
        rendered: Some(error.display_with_source(source)),
        line: error.span.as_ref().map(|span| span.start.line),
        column: error.span.as_ref().map(|span| span.start.column),
    })?;

    let mut parser = StmtParser::new_with_spans(&tokens, &spans);
    let program = parser
        .parse_program_with_enhanced_errors(source)
        .map_err(|error| Diagnostic {
            level: "error",
            message: error.to_string(),
            rendered: Some(error.display_with_source(source)),
            line: error.span.as_ref().map(|span| span.start.line),
            column: error.span.as_ref().map(|span| span.start.column),
        })?;

    let mut registry = ModuleRegistry::new();
    lk_stdlib_web::register_web_stdlib(&mut registry).map_err(runtime_diagnostic)?;
    let resolver = Arc::new(ModuleResolver::with_registry(registry));
    let mut ctx = VmContext::new()
        .with_resolver(resolver)
        .with_type_checker(Some(TypeChecker::new_strict()));
    let result = execute_program_with_ctx_and_budget(&program, &mut ctx, DEFAULT_INSTRUCTION_BUDGET)
        .map_err(runtime_diagnostic)?;
    runtime_display_value(result.first_return(), result.state.heap()).map_err(runtime_diagnostic)
}

fn runtime_diagnostic(error: impl std::fmt::Display) -> Diagnostic {
    Diagnostic {
        level: "error",
        message: error.to_string(),
        rendered: None,
        line: None,
        column: None,
    }
}

#[cfg(test)]
mod tests {
    use super::run_lk_inner;

    #[test]
    fn runs_trait_methods_in_wasm_context() {
        let source = r#"
struct Rect { w: Int, h: Int }

trait Area {
  fn area(self) -> Int;
}

impl Area for Rect {
  fn area(self) -> Int { return self.w * self.h; }
}

let shape = Rect { w: 8, h: 5 };
return shape.area();
"#;

        assert_eq!(run_lk_inner(source).expect("trait example should run"), "40");
    }
}
