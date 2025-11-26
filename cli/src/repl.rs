use rustyline::{DefaultEditor, error::ReadlineError};
use std::sync::Arc;

use lkr_core::{
    module::ModuleRegistry,
    rt,
    stmt::{ModuleResolver, StmtParser},
    token::Tokenizer,
    typ::TypeChecker,
    val::Val,
    vm::{Vm, VmContext, compile_program},
};

fn print_repl_help() {
    eprintln!("Commands: :quit | :exit | :q, :help");
}

fn should_continue_multiline(buf: &str) -> bool {
    // Simple bracket/brace/paren balance check; continue if unbalanced or trailing '\\'
    let mut paren = 0i32;
    let mut brace = 0i32;
    let mut bracket = 0i32;
    for ch in buf.chars() {
        match ch {
            '(' => paren += 1,
            ')' => paren -= 1,
            '{' => brace += 1,
            '}' => brace -= 1,
            '[' => bracket += 1,
            ']' => bracket -= 1,
            _ => {}
        }
    }
    let trailing_backslash = buf.trim_end().ends_with('\\');
    paren > 0 || brace > 0 || bracket > 0 || trailing_backslash
}

// Normalize bare expressions to avoid tokenizer merging '+/-' with following digits
// when used as binary operators without spaces, e.g., "a+1" -> "a+ 1", "a-1" -> "a- 1".
// This only tweaks outside of quoted strings.
fn normalize_binary_signs(src: &str) -> String {
    let mut out = String::with_capacity(src.len() + 8);
    let chars: Vec<char> = src.chars().collect();
    let mut i = 0usize;
    let len = chars.len();
    let mut in_single = false;
    let mut in_double = false;
    while i < len {
        let c = chars[i];
        if !in_single && c == '"' {
            in_double = !in_double;
            out.push(c);
            i += 1;
            continue;
        }
        if !in_double && c == '\'' {
            in_single = !in_single;
            out.push(c);
            i += 1;
            continue;
        }
        if in_single || in_double {
            out.push(c);
            i += 1;
            continue;
        }

        if (c == '+' || c == '-') && i + 1 < len && chars[i + 1].is_ascii_digit() {
            // Look at previous visible char to decide if this is binary op context
            let mut j = i as isize - 1;
            let mut prev: Option<char> = None;
            while j >= 0 {
                let pj = chars[j as usize];
                if pj.is_whitespace() {
                    j -= 1;
                    continue;
                }
                prev = Some(pj);
                break;
            }
            let prev_is_value_like = matches!(
                prev,
                Some(ch)
                    if ch.is_ascii_alphanumeric()
                        || ch == '_'
                        || ch == ')'
                        || ch == ']'
                        || ch == '}'
                        || ch == '"'
                        || ch == '\''
            );

            if prev_is_value_like {
                // Insert a space after '+' or '-' to force binary tokenization
                out.push(c);
                out.push(' ');
                i += 1;
                continue;
            }
        }

        out.push(c);
        i += 1;
    }
    out
}

pub fn run(_is_statement_mode: bool) -> anyhow::Result<()> {
    // Initialize runtime

    if let Err(e) = rt::init_runtime() {
        eprintln!("Warning: Failed to initialize runtime: {}", e);
    }

    // Prepare stdlib and environment (persist across statements)
    let mut registry = ModuleRegistry::new();
    lkr_stdlib::register_stdlib_globals(&mut registry);
    lkr_stdlib::register_stdlib_modules(&mut registry)?;
    let resolver = Arc::new(ModuleResolver::with_registry(registry));
    let mut env = VmContext::new()
        .with_resolver(resolver)
        .with_type_checker(Some(TypeChecker::new_strict()));
    let mut vm = Vm::new();

    // In-memory line editor with history and arrow key support
    let mut rl = DefaultEditor::new()?;
    let mut buffer = String::new();

    print_repl_help();

    loop {
        // Prompt
        buffer.clear();
        let mut acc = String::new();
        // Read one or more lines using rustyline until complete
        loop {
            let prompt = if acc.is_empty() { "> " } else { "... " };
            match rl.readline(prompt) {
                Ok(line) => {
                    let trimmed = line.trim_end();

                    // Commands only when starting fresh
                    if acc.is_empty() && trimmed.starts_with(':') {
                        match trimmed {
                            ":quit" | ":exit" | ":q" => {
                                rt::shutdown_runtime();
                                return Ok(());
                            }
                            ":help" => {
                                print_repl_help();
                                acc.clear();
                                break; // show new prompt
                            }
                            _ => {
                                eprintln!("Unknown command. Type :help for help.");
                                acc.clear();
                                break; // new prompt
                            }
                        }
                    }

                    // Support line continuation via trailing '\\' (strip it)
                    if trimmed.ends_with('\\') {
                        acc.push_str(trimmed.strip_suffix('\\').unwrap_or(trimmed));
                        acc.push('\n');
                        continue;
                    }

                    acc.push_str(trimmed);
                    acc.push('\n');
                    if !should_continue_multiline(&acc) {
                        break;
                    }
                }
                Err(ReadlineError::Interrupted) => {
                    // Ctrl-C: clear current buffer and prompt again
                    acc.clear();
                    eprintln!("^C");
                    break;
                }
                Err(ReadlineError::Eof) => {
                    // Ctrl-D: exit if nothing pending; otherwise treat as submit
                    if acc.trim().is_empty() {
                        println!();
                        rt::shutdown_runtime();
                        return Ok(());
                    } else {
                        break;
                    }
                }
                Err(e) => {
                    eprintln!("Readline error: {}", e);
                    continue;
                }
            }
        }

        let final_src = acc.trim_end().to_string();
        if final_src.trim().is_empty() {
            continue;
        }
        // Add to in-memory history
        let _ = rl.add_history_entry(final_src.as_str());

        // Always run in statement mode. If parsing as statements fails,
        // try wrapping input as println(<expr>); to allow quick expression prints.
        let result = {
            let src = final_src.clone();
            let (tokens, spans) = match Tokenizer::tokenize_enhanced_with_spans(&src) {
                Ok((tokens, spans)) => (tokens, spans),
                Err(parse_err) => {
                    eprintln!("Error: {}", parse_err);
                    continue;
                }
            };

            let mut parser = StmtParser::new_with_spans(&tokens, &spans);
            match parser.parse_program_with_enhanced_errors(&src) {
                Ok(program) => {
                    let function = compile_program(&program);
                    vm.exec_with(&function, &mut env, None)
                }
                Err(_parse_err) => {
                    // Attempt to treat input as expression: println((<src>));
                    // Normalize to avoid tokenizer merging '+'/'-' with following digits in binary contexts.
                    let normalized = normalize_binary_signs(&src);
                    let wrapped = format!("println(({}));", normalized);
                    match Tokenizer::tokenize_enhanced_with_spans(&wrapped) {
                        Ok((wtoks, wspans)) => {
                            let mut wparser = StmtParser::new_with_spans(&wtoks, &wspans);
                            match wparser.parse_program_with_enhanced_errors(&wrapped) {
                                Ok(wprog) => {
                                    let function = compile_program(&wprog);
                                    vm.exec_with(&function, &mut env, None)
                                }
                                Err(perr) => {
                                    eprintln!("Error: {}", perr);
                                    continue;
                                }
                            }
                        }
                        Err(terr) => {
                            eprintln!("Error: {}", terr);
                            continue;
                        }
                    }
                }
            }
        };

        match result {
            Ok(res) => {
                if !matches!(res, Val::Nil) {
                    println!("{}", res.display_string(Some(&env)));
                }
            }
            Err(e) => eprintln!("Error: {}", e),
        }
    }
}
