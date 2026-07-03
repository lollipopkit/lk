// The parser's AST hands statements around as `Vec<Box<Stmt>>`; keeping the
// macro system on the same shape avoids re-boxing at every parse boundary.
#![allow(clippy::vec_box, clippy::boxed_local)]
use super::{proc_deps::ProcMacroDependencyRecorder, proc_output::parse_tokens_from_proc_output};
#[cfg(not(feature = "std"))]
use crate::compat::prelude::*;
#[cfg_attr(not(feature = "std"), allow(dead_code, unused_imports))]
mod derive;
mod origins;

use self::origins::{generated_item_origins, stmt_label};
use crate::compat::collections::{HashMap, HashSet};
use crate::compat::path::PathBuf;
use crate::{
    macro_system::token_lexeme,
    stmt::{Attribute, Program, Stmt, StmtParser},
    token::{ParseError, Position, Span, Token, Tokenizer},
};
use core::fmt;
use core::sync::atomic::{AtomicU64, Ordering};
use core::time::Duration;
use serde::{Deserialize, Serialize};
// Running an external proc-macro provider needs a process + filesystem, which
// don't exist under no_std; the spawn path is std-only (plan M0.7/8).
#[cfg(feature = "std")]
use std::{
    fs::{self, File},
    io::Write,
    process::{Command, Stdio},
    time::{Instant, SystemTime, UNIX_EPOCH},
};

pub const PROC_MACRO_PROTOCOL_VERSION: u32 = 1;
const BUILTIN_SHOW_TRAIT: &str = "__LKShow";
static PROC_MACRO_TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProcMacroKind {
    FunctionLike,
    Derive,
    Attribute,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcMacroSpan {
    pub start_line: u32,
    pub start_column: u32,
    pub start_offset: usize,
    pub end_line: u32,
    pub end_column: u32,
    pub end_offset: usize,
}

impl From<&Span> for ProcMacroSpan {
    fn from(span: &Span) -> Self {
        Self {
            start_line: span.start.line,
            start_column: span.start.column,
            start_offset: span.start.offset,
            end_line: span.end.line,
            end_column: span.end.column,
            end_offset: span.end.offset,
        }
    }
}

impl ProcMacroSpan {
    pub fn to_span(&self) -> Span {
        Span::new(
            Position::new(self.start_line, self.start_column, self.start_offset),
            Position::new(self.end_line, self.end_column, self.end_offset),
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcMacroToken {
    pub kind: String,
    pub lexeme: String,
    pub span: Option<ProcMacroSpan>,
}

impl ProcMacroToken {
    pub fn from_token(token: &Token, span: Option<&Span>) -> Self {
        Self {
            kind: token_kind(token),
            lexeme: token_lexeme(token),
            span: span.map(ProcMacroSpan::from),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcMacroRequest {
    pub protocol_version: u32,
    pub kind: ProcMacroKind,
    pub macro_name: String,
    pub input_tokens: Vec<ProcMacroToken>,
    pub item_tokens: Vec<ProcMacroToken>,
    pub package: Option<String>,
    pub module: Option<String>,
    pub features: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcMacroResponse {
    pub protocol_version: u32,
    pub output_tokens: Vec<ProcMacroToken>,
    pub diagnostics: Vec<ProcMacroDiagnostic>,
    pub dependencies: Vec<ProcMacroDependency>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcMacroDiagnostic {
    pub level: ProcMacroDiagnosticLevel,
    pub message: String,
    pub span: Option<ProcMacroSpan>,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProcMacroDiagnosticLevel {
    Error,
    Warning,
    Note,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcMacroDependency {
    pub path: String,
    pub digest: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ProcMacroProcessConfig {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub timeout: Duration,
    pub max_output_bytes: usize,
    pub env: Option<Vec<(String, String)>>,
}

impl ProcMacroProcessConfig {
    pub fn new(program: impl Into<PathBuf>) -> Self {
        Self {
            program: program.into(),
            ..Self::default()
        }
    }
}

impl Default for ProcMacroProcessConfig {
    fn default() -> Self {
        Self {
            program: PathBuf::new(),
            args: Vec::new(),
            timeout: Duration::from_secs(5),
            max_output_bytes: 1_048_576,
            env: None,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct ProcMacroProviders {
    derive: HashMap<String, ProcMacroProcessConfig>,
    attribute: HashMap<String, ProcMacroProcessConfig>,
    function_like: HashMap<String, ProcMacroProcessConfig>,
}

impl ProcMacroProviders {
    pub fn register_derive(&mut self, name: impl Into<String>, config: ProcMacroProcessConfig) {
        self.derive.insert(name.into(), config);
    }

    pub fn register_attribute(&mut self, name: impl Into<String>, config: ProcMacroProcessConfig) {
        self.attribute.insert(name.into(), config);
    }

    pub fn register_function_like(&mut self, name: impl Into<String>, config: ProcMacroProcessConfig) {
        self.function_like.insert(name.into(), config);
    }

    pub fn derive_provider(&self, name: &str) -> Option<&ProcMacroProcessConfig> {
        self.derive.get(name)
    }

    pub fn attribute_provider(&self, name: &str) -> Option<&ProcMacroProcessConfig> {
        self.attribute.get(name)
    }

    pub fn function_like_provider(&self, name: &str) -> Option<&ProcMacroProcessConfig> {
        self.function_like.get(name)
    }

    pub fn register_trusted_dependency(&mut self, dependency: &str, providers: ProcMacroProviders) {
        for (name, config) in providers.derive {
            self.derive.entry(name).or_insert(config);
        }
        for (name, config) in providers.attribute {
            self.attribute.entry(name).or_insert(config);
        }
        for (name, config) in providers.function_like {
            self.function_like
                .entry(format!("{dependency}::{name}"))
                .or_insert(config);
        }
    }
}

#[derive(Debug)]
pub enum ProcMacroProcessError {
    EmptyProgram,
    #[cfg(feature = "std")]
    Spawn(std::io::Error),
    #[cfg(feature = "std")]
    WriteStdin(std::io::Error),
    #[cfg(feature = "std")]
    Wait(std::io::Error),
    Timeout {
        timeout: Duration,
    },
    Exit {
        status: String,
        stderr: String,
    },
    OutputTooLarge {
        max: usize,
        actual: u64,
    },
    #[cfg(feature = "std")]
    ReadOutput(std::io::Error),
    Decode(serde_json::Error),
    ProtocolVersion {
        expected: u32,
        actual: u32,
    },
}

impl fmt::Display for ProcMacroProcessError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyProgram => write!(f, "procedural macro process program is empty"),
            #[cfg(feature = "std")]
            Self::Spawn(err) => write!(f, "failed to spawn procedural macro process: {err}"),
            #[cfg(feature = "std")]
            Self::WriteStdin(err) => write!(f, "failed to write procedural macro request: {err}"),
            #[cfg(feature = "std")]
            Self::Wait(err) => write!(f, "failed while waiting for procedural macro process: {err}"),
            Self::Timeout { timeout } => write!(f, "procedural macro process timed out after {timeout:?}"),
            Self::Exit { status, stderr } => {
                if stderr.trim().is_empty() {
                    write!(f, "procedural macro process exited with {status}")
                } else {
                    write!(f, "procedural macro process exited with {status}: {}", stderr.trim())
                }
            }
            Self::OutputTooLarge { max, actual } => {
                write!(f, "procedural macro output exceeded {max} bytes: {actual} bytes")
            }
            #[cfg(feature = "std")]
            Self::ReadOutput(err) => write!(f, "failed to read procedural macro output: {err}"),
            Self::Decode(err) => write!(f, "failed to decode procedural macro response: {err}"),
            Self::ProtocolVersion { expected, actual } => write!(
                f,
                "procedural macro protocol version mismatch: expected {expected}, got {actual}"
            ),
        }
    }
}

impl core::error::Error for ProcMacroProcessError {}

#[derive(Debug, Clone, Default)]
pub struct ProcMacroOptions {
    pub package: Option<String>,
    pub module: Option<String>,
    pub features: Vec<String>,
    pub providers: ProcMacroProviders,
    pub dependency_recorder: ProcMacroDependencyRecorder,
}

#[derive(Debug, Default)]
struct AstMacroState {
    show_trait_available: bool,
    origins: Vec<AstMacroOrigin>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AstMacroExpansionResult {
    pub program: Program,
    pub origins: Vec<AstMacroOrigin>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AstMacroOrigin {
    pub macro_name: String,
    pub kind: AstMacroOriginKind,
    pub input_span: Option<Span>,
    pub generated_items: usize,
    pub generated_item_labels: Vec<String>,
    pub generated_item_origins: Vec<AstGeneratedItemOrigin>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AstGeneratedItemOrigin {
    pub label: String,
    pub span: Option<Span>,
    pub generated_member_origins: Vec<AstGeneratedMemberOrigin>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AstGeneratedMemberOrigin {
    pub label: String,
    pub span: Option<Span>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AstMacroOriginKind {
    Cfg,
    BuiltinDerive,
    ExternalDerive,
    Attribute,
}

impl AstMacroOriginKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Cfg => "cfg",
            Self::BuiltinDerive => "builtin_derive",
            Self::ExternalDerive => "external_derive",
            Self::Attribute => "attribute",
        }
    }
}

#[cfg(feature = "std")]
pub fn run_proc_macro_process(
    request: &ProcMacroRequest,
    config: &ProcMacroProcessConfig,
) -> Result<ProcMacroResponse, ProcMacroProcessError> {
    if config.program.as_os_str().is_empty() {
        return Err(ProcMacroProcessError::EmptyProgram);
    }

    let output_paths = TempProcMacroOutput::new();
    let stdout = File::create(&output_paths.stdout).map_err(ProcMacroProcessError::Spawn)?;
    let stderr = File::create(&output_paths.stderr).map_err(ProcMacroProcessError::Spawn)?;
    let mut child = Command::new(&config.program)
        .args(&config.args)
        .envs(
            config
                .env
                .iter()
                .flat_map(|pairs| pairs.iter().map(|(k, v)| (k.as_str(), v.as_str()))),
        )
        .stdin(Stdio::piped())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr))
        .spawn()
        .map_err(ProcMacroProcessError::Spawn)?;

    let request_bytes = serde_json::to_vec(request).map_err(ProcMacroProcessError::Decode)?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(&request_bytes)
            .map_err(ProcMacroProcessError::WriteStdin)?;
    }

    let started = Instant::now();
    let mut backoff = Duration::from_millis(5);
    let max_backoff = Duration::from_millis(100);
    let status = loop {
        if let Some(status) = child.try_wait().map_err(ProcMacroProcessError::Wait)? {
            break status;
        }
        if started.elapsed() >= config.timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Err(ProcMacroProcessError::Timeout {
                timeout: config.timeout,
            });
        }
        std::thread::sleep(backoff);
        backoff = (backoff * 2).min(max_backoff);
    };

    let stderr = read_limited_file(&output_paths.stderr, config.max_output_bytes)?;
    if !status.success() {
        return Err(ProcMacroProcessError::Exit {
            status: status.to_string(),
            stderr: String::from_utf8_lossy(&stderr).into_owned(),
        });
    }

    let stdout = read_limited_file(&output_paths.stdout, config.max_output_bytes)?;
    let response: ProcMacroResponse = serde_json::from_slice(&stdout).map_err(ProcMacroProcessError::Decode)?;
    if response.protocol_version != PROC_MACRO_PROTOCOL_VERSION {
        return Err(ProcMacroProcessError::ProtocolVersion {
            expected: PROC_MACRO_PROTOCOL_VERSION,
            actual: response.protocol_version,
        });
    }
    Ok(response)
}

#[cfg(feature = "std")]
struct TempProcMacroOutput {
    stdout: PathBuf,
    stderr: PathBuf,
}

#[cfg(feature = "std")]
impl TempProcMacroOutput {
    fn new() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let counter = PROC_MACRO_TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let base = std::env::temp_dir().join(format!("lk-proc-macro-{}-{nonce}-{counter}", std::process::id()));
        Self {
            stdout: base.with_extension("stdout.json"),
            stderr: base.with_extension("stderr.txt"),
        }
    }
}

#[cfg(feature = "std")]
impl Drop for TempProcMacroOutput {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.stdout);
        let _ = fs::remove_file(&self.stderr);
    }
}

#[cfg(feature = "std")]
fn read_limited_file(path: &PathBuf, max_bytes: usize) -> Result<Vec<u8>, ProcMacroProcessError> {
    let metadata = fs::metadata(path).map_err(ProcMacroProcessError::ReadOutput)?;
    if metadata.len() > max_bytes as u64 {
        return Err(ProcMacroProcessError::OutputTooLarge {
            max: max_bytes,
            actual: metadata.len(),
        });
    }
    fs::read(path).map_err(ProcMacroProcessError::ReadOutput)
}

pub fn expand_ast_macros(program: Program, options: ProcMacroOptions) -> Result<Program, ParseError> {
    Ok(expand_ast_macros_with_metadata(program, options)?.program)
}

pub fn expand_ast_macros_with_metadata(
    program: Program,
    options: ProcMacroOptions,
) -> Result<AstMacroExpansionResult, ParseError> {
    let mut state = AstMacroState::default();
    let statements = expand_stmt_vec(program.statements, &options, &mut state)?;
    let program = Program::new(statements).map_err(|err| ParseError::new(err.to_string()))?;
    Ok(AstMacroExpansionResult {
        program,
        origins: state.origins,
    })
}

fn expand_stmt_vec(
    statements: Vec<Box<Stmt>>,
    options: &ProcMacroOptions,
    state: &mut AstMacroState,
) -> Result<Vec<Box<Stmt>>, ParseError> {
    let mut expanded = Vec::with_capacity(statements.len());
    for stmt in statements {
        expanded.extend(expand_stmt(*stmt, options, state)?);
    }
    Ok(expanded)
}

fn expand_stmt(
    stmt: Stmt,
    options: &ProcMacroOptions,
    state: &mut AstMacroState,
) -> Result<Vec<Box<Stmt>>, ParseError> {
    match stmt {
        Stmt::Attributed { attributes, item } => expand_attributed_stmt(attributes, *item, options, state),
        stmt => Ok(vec![Box::new(expand_stmt_no_split(stmt, options, state)?)]),
    }
}

fn expand_stmt_no_split(stmt: Stmt, options: &ProcMacroOptions, state: &mut AstMacroState) -> Result<Stmt, ParseError> {
    match stmt {
        Stmt::If {
            condition,
            then_stmt,
            else_stmt,
        } => Ok(Stmt::If {
            condition,
            then_stmt: expand_stmt_box_to_single(then_stmt, options, state)?,
            else_stmt: else_stmt
                .map(|stmt| expand_stmt_box_to_single(stmt, options, state))
                .transpose()?,
        }),
        Stmt::IfLet {
            pattern,
            value,
            then_stmt,
            else_stmt,
        } => Ok(Stmt::IfLet {
            pattern,
            value,
            then_stmt: expand_stmt_box_to_single(then_stmt, options, state)?,
            else_stmt: else_stmt
                .map(|stmt| expand_stmt_box_to_single(stmt, options, state))
                .transpose()?,
        }),
        Stmt::While { condition, body } => Ok(Stmt::While {
            condition,
            body: expand_stmt_box_to_single(body, options, state)?,
        }),
        Stmt::WhileLet { pattern, value, body } => Ok(Stmt::WhileLet {
            pattern,
            value,
            body: expand_stmt_box_to_single(body, options, state)?,
        }),
        Stmt::For {
            pattern,
            iterable,
            body,
        } => Ok(Stmt::For {
            pattern,
            iterable,
            body: expand_stmt_box_to_single(body, options, state)?,
        }),
        Stmt::Function {
            name,
            params,
            param_types,
            named_params,
            return_type,
            body,
        } => Ok(Stmt::Function {
            name,
            params,
            param_types,
            named_params,
            return_type,
            body: expand_stmt_box_to_single(body, options, state)?,
        }),
        Stmt::Impl {
            trait_name,
            target_type,
            methods,
        } => {
            let mut expanded_methods = Vec::with_capacity(methods.len());
            for method in methods {
                let expanded = expand_stmt(method, options, state)?;
                if expanded.len() > 1 {
                    return Err(ParseError::new(
                        "Attribute macros on impl methods must expand to at most one method".to_string(),
                    ));
                }
                let Some(method) = expanded.into_iter().next() else {
                    continue;
                };
                if !is_impl_method_stmt(&method) {
                    return Err(ParseError::new(
                        "Attribute macros on impl methods must expand to a function method".to_string(),
                    ));
                }
                expanded_methods.push(*method);
            }
            Ok(Stmt::Impl {
                trait_name,
                target_type,
                methods: expanded_methods,
            })
        }
        Stmt::Block { statements } => Ok(Stmt::Block {
            statements: expand_stmt_vec(statements, options, state)?,
        }),
        other => {
            if matches!(other, Stmt::Trait { ref name, .. } if name == BUILTIN_SHOW_TRAIT) {
                state.show_trait_available = true;
            }
            Ok(other)
        }
    }
}

fn is_impl_method_stmt(stmt: &Stmt) -> bool {
    match stmt {
        Stmt::Function { .. } => true,
        Stmt::Attributed { item, .. } => is_impl_method_stmt(item),
        _ => false,
    }
}

fn expand_stmt_box_to_single(
    stmt: Box<Stmt>,
    options: &ProcMacroOptions,
    state: &mut AstMacroState,
) -> Result<Box<Stmt>, ParseError> {
    let expanded = expand_stmt(*stmt, options, state)?;
    if expanded.is_empty() {
        return Ok(Box::new(Stmt::Block { statements: Vec::new() }));
    }
    if expanded.len() == 1 {
        return Ok(expanded.into_iter().next().expect("single expanded statement"));
    }
    Ok(Box::new(Stmt::Block { statements: expanded }))
}

fn expand_attributed_stmt(
    attributes: Vec<Attribute>,
    item: Stmt,
    options: &ProcMacroOptions,
    state: &mut AstMacroState,
) -> Result<Vec<Box<Stmt>>, ParseError> {
    let mut preserved_attrs = Vec::new();
    let mut attribute_macros = Vec::new();
    let mut derives = Vec::new();
    let mut derive_span = None;
    let features: HashSet<&str> = options.features.iter().map(String::as_str).collect();
    for attr in attributes {
        if let Some(enabled) = parse_cfg_attribute(&attr, &features)? {
            if !enabled {
                record_ast_origin(state, "cfg", AstMacroOriginKind::Cfg, attr.span.clone(), &[]);
                return Ok(Vec::new());
            }
        } else if let Some(parsed) = derive::parse_derive_attribute(&attr)? {
            if derive_span.is_none() {
                derive_span = attr.span.clone();
            }
            for derive in parsed {
                if !derives.contains(&derive) {
                    derives.push(derive);
                }
            }
        } else if let Some(name) = registered_attribute_macro_name(&attr, options) {
            attribute_macros.push((name, attr));
        } else {
            preserved_attrs.push(attr);
        }
    }

    let expanded_item = expand_stmt_no_split(item, options, state)?;
    let mut transformed_items = vec![Box::new(expanded_item)];
    for (name, attr) in attribute_macros {
        if transformed_items.len() != 1 {
            return Err(error_at_attr(
                &attr,
                &format!("Attribute macro `{name}` can only transform one item at this stage"),
            ));
        }
        let item = *transformed_items.pop().expect("attribute macro transform has one item");
        transformed_items = expand_external_attribute(&name, &attr, item, options)?;
        record_ast_origin(
            state,
            &name,
            AstMacroOriginKind::Attribute,
            attr.span.clone(),
            &transformed_items,
        );
    }

    if derives.is_empty() {
        if transformed_items.len() != 1 {
            if preserved_attrs.is_empty() {
                return Ok(transformed_items);
            }
            return Err(error_from_span(
                preserved_attrs.first().and_then(|attr| attr.span.as_ref()),
                "Preserved attributes cannot be applied after an attribute macro expands to multiple items",
            ));
        }
        let expanded_item = *transformed_items
            .pop()
            .expect("single transformed item with no derives");
        return Ok(vec![Box::new(apply_preserved_attributes(
            preserved_attrs,
            expanded_item,
        ))]);
    }

    if transformed_items.len() != 1 {
        return Err(error_from_span(
            derive_span.as_ref(),
            "derive macros require a single item after attribute macro expansion",
        ));
    }
    let expanded_item = *transformed_items.pop().expect("single transformed item with derives");

    derive::expand_derives(derives, derive_span, preserved_attrs, expanded_item, options, state)
}

fn record_ast_origin(
    state: &mut AstMacroState,
    macro_name: &str,
    kind: AstMacroOriginKind,
    input_span: Option<Span>,
    generated_items: &[Box<Stmt>],
) {
    let generated_item_origins = generated_item_origins(generated_items, input_span.clone());
    state.origins.push(AstMacroOrigin {
        macro_name: macro_name.to_string(),
        kind,
        input_span,
        generated_items: generated_items.len(),
        generated_item_labels: generated_items.iter().map(|stmt| stmt_label(stmt)).collect(),
        generated_item_origins,
    });
}

fn registered_attribute_macro_name(attr: &Attribute, options: &ProcMacroOptions) -> Option<String> {
    let Some(Token::Id(name)) = attr.tokens.first() else {
        return None;
    };
    options.providers.attribute_provider(name).map(|_| name.clone())
}

fn parse_cfg_attribute(attr: &Attribute, features: &HashSet<&str>) -> Result<Option<bool>, ParseError> {
    let Some(Token::Id(name)) = attr.tokens.first() else {
        return Ok(None);
    };
    if name != "cfg" {
        return Ok(None);
    }
    if attr.tokens.len() < 3 || attr.tokens.get(1) != Some(&Token::LParen) || attr.tokens.last() != Some(&Token::RParen)
    {
        return Err(error_at_attr(
            attr,
            "Malformed cfg attribute; expected #[cfg(predicate)]",
        ));
    }
    let mut parser = CfgParser {
        tokens: &attr.tokens[2..attr.tokens.len() - 1],
        pos: 0,
        features,
        attr,
    };
    let enabled = parser.parse_expr()?;
    if !parser.eof() {
        return Err(error_at_attr(attr, "Unexpected tokens after cfg predicate"));
    }
    Ok(Some(enabled))
}

struct CfgParser<'a> {
    tokens: &'a [Token],
    pos: usize,
    features: &'a HashSet<&'a str>,
    attr: &'a Attribute,
}

impl<'a> CfgParser<'a> {
    fn parse_expr(&mut self) -> Result<bool, ParseError> {
        match self.peek() {
            Some(Token::Bool(value)) => {
                let value = *value;
                self.pos += 1;
                Ok(value)
            }
            Some(Token::Id(name)) if name == "feature" => self.parse_feature(),
            Some(Token::Id(name)) if name == "not" => self.parse_not(),
            Some(Token::Id(name)) if name == "any" => self.parse_any(),
            Some(Token::Id(name)) if name == "all" => self.parse_all(),
            Some(_) => Err(error_at_attr(self.attr, "Expected cfg predicate")),
            None => Err(error_at_attr(self.attr, "cfg predicate cannot be empty")),
        }
    }

    fn parse_feature(&mut self) -> Result<bool, ParseError> {
        self.expect_id("feature")?;
        if self.consume(&Token::Assign) {
            let feature = self.expect_string("Expected feature name after '='")?;
            return Ok(self.features.contains(feature.as_str()));
        }
        self.expect(&Token::LParen, "Expected '=' or '(' after feature")?;
        let feature = self.expect_string("Expected feature name inside feature(...)")?;
        self.expect(&Token::RParen, "Expected ')' after feature name")?;
        Ok(self.features.contains(feature.as_str()))
    }

    fn parse_not(&mut self) -> Result<bool, ParseError> {
        self.expect_id("not")?;
        self.expect(&Token::LParen, "Expected '(' after not")?;
        let value = !self.parse_expr()?;
        self.expect(&Token::RParen, "Expected ')' after not predicate")?;
        Ok(value)
    }

    fn parse_any(&mut self) -> Result<bool, ParseError> {
        self.expect_id("any")?;
        self.parse_list("any", false, |values| values.iter().any(|value| *value))
    }

    fn parse_all(&mut self) -> Result<bool, ParseError> {
        self.expect_id("all")?;
        self.parse_list("all", true, |values| values.iter().all(|value| *value))
    }

    fn parse_list(
        &mut self,
        name: &str,
        empty_value: bool,
        fold: impl FnOnce(&[bool]) -> bool,
    ) -> Result<bool, ParseError> {
        self.expect(&Token::LParen, &format!("Expected '(' after {name}"))?;
        if self.consume(&Token::RParen) {
            return Ok(empty_value);
        }
        let mut values = Vec::new();
        loop {
            values.push(self.parse_expr()?);
            if self.consume(&Token::Comma) {
                if self.consume(&Token::RParen) {
                    break;
                }
                continue;
            }
            self.expect(&Token::RParen, &format!("Expected ',' or ')' in {name} predicate"))?;
            break;
        }
        Ok(fold(&values))
    }

    fn eof(&self) -> bool {
        self.pos >= self.tokens.len()
    }

    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
    }

    fn consume(&mut self, expected: &Token) -> bool {
        if self.peek() == Some(expected) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn expect(&mut self, expected: &Token, message: &str) -> Result<(), ParseError> {
        if self.consume(expected) {
            Ok(())
        } else {
            Err(error_at_attr(self.attr, message))
        }
    }

    fn expect_id(&mut self, expected: &str) -> Result<(), ParseError> {
        match self.peek() {
            Some(Token::Id(name)) if name == expected => {
                self.pos += 1;
                Ok(())
            }
            _ => Err(error_at_attr(
                self.attr,
                &format!("Expected cfg predicate `{expected}`"),
            )),
        }
    }

    fn expect_string(&mut self, message: &str) -> Result<String, ParseError> {
        match self.peek() {
            Some(Token::Str(value)) => {
                let value = value.clone();
                self.pos += 1;
                Ok(value)
            }
            _ => Err(error_at_attr(self.attr, message)),
        }
    }
}

fn apply_preserved_attributes(attributes: Vec<Attribute>, item: Stmt) -> Stmt {
    if attributes.is_empty() {
        item
    } else {
        Stmt::Attributed {
            attributes,
            item: Box::new(item),
        }
    }
}

fn expand_external_attribute(
    name: &str,
    attr: &Attribute,
    item: Stmt,
    options: &ProcMacroOptions,
) -> Result<Vec<Box<Stmt>>, ParseError> {
    let Some(config) = options.providers.attribute_provider(name) else {
        return Err(error_at_attr(
            attr,
            &format!("No procedural attribute provider registered for `{name}`"),
        ));
    };
    // Invoking an external provider needs a process, which no_std lacks (M0.7/8).
    #[cfg(not(feature = "std"))]
    {
        let _ = (config, item);
        Err(error_at_attr(
            attr,
            &format!("Procedural attribute `{name}` requires the std feature"),
        ))
    }
    #[cfg(feature = "std")]
    {
        let request = attribute_request(name, attr, &item, options)?;
        let response = run_proc_macro_process(&request, config)
            .map_err(|err| error_at_attr(attr, &format!("Procedural attribute `{name}` failed: {err}")))?;
        reject_error_diagnostics(name, &response.diagnostics, attr.span.as_ref())?;
        options.dependency_recorder.record(&response.dependencies);
        parse_proc_macro_output_items(name, &response.output_tokens, attr.span.as_ref())
    }
}

fn attribute_request(
    macro_name: &str,
    attr: &Attribute,
    item: &Stmt,
    options: &ProcMacroOptions,
) -> Result<ProcMacroRequest, ParseError> {
    Ok(ProcMacroRequest {
        protocol_version: PROC_MACRO_PROTOCOL_VERSION,
        kind: ProcMacroKind::Attribute,
        macro_name: macro_name.to_string(),
        input_tokens: attribute_input_tokens(attr),
        item_tokens: stmt_item_tokens(item, attr.span.as_ref(), macro_name)?,
        package: options.package.clone(),
        module: options.module.clone(),
        features: options.features.clone(),
    })
}

fn reject_error_diagnostics(
    macro_name: &str,
    diagnostics: &[ProcMacroDiagnostic],
    fallback_span: Option<&Span>,
) -> Result<(), ParseError> {
    let Some(diagnostic) = diagnostics
        .iter()
        .find(|diagnostic| diagnostic.level == ProcMacroDiagnosticLevel::Error)
    else {
        return Ok(());
    };
    let mut message = format!(
        "Procedural macro `{macro_name}` reported an error: {}",
        diagnostic.message
    );
    if !diagnostic.notes.is_empty() {
        message.push_str("; notes: ");
        message.push_str(&diagnostic.notes.join("; "));
    }
    let span = diagnostic
        .span
        .as_ref()
        .map(ProcMacroSpan::to_span)
        .or_else(|| fallback_span.cloned());
    if let Some(span) = span {
        Err(ParseError::with_span(message, span))
    } else {
        Err(ParseError::new(message))
    }
}

fn parse_proc_macro_output_items(
    macro_name: &str,
    tokens: &[ProcMacroToken],
    fallback_span: Option<&Span>,
) -> Result<Vec<Box<Stmt>>, ParseError> {
    if tokens.is_empty() {
        return Ok(Vec::new());
    }
    let (source, tokens, spans) = parse_tokens_from_proc_output(macro_name, tokens, fallback_span)?;
    let mut parser = StmtParser::new_with_spans(&tokens, &spans);
    let program = parser
        .parse_program_with_enhanced_errors(&source)
        .map_err(|err| proc_output_parse_error(macro_name, err, fallback_span))?;
    Ok(program.statements)
}

fn proc_output_parse_error(macro_name: &str, err: ParseError, fallback_span: Option<&Span>) -> ParseError {
    let message = format!("Procedural macro `{macro_name}` output did not parse: {err}");
    if let Some(span) = err.span {
        ParseError::with_span(message, span)
    } else {
        error_from_span(fallback_span, &message)
    }
}

fn attribute_input_tokens(attr: &Attribute) -> Vec<ProcMacroToken> {
    attr.tokens
        .iter()
        .map(|token| ProcMacroToken::from_token(token, attr.span.as_ref()))
        .collect()
}

fn stmt_item_tokens(
    item: &Stmt,
    fallback_span: Option<&Span>,
    macro_name: &str,
) -> Result<Vec<ProcMacroToken>, ParseError> {
    let source = item.to_string();
    let tokens = Tokenizer::tokenize(&source).map_err(|err| {
        error_from_span(
            fallback_span,
            &format!("Procedural macro `{macro_name}` could not encode input item tokens: {err}"),
        )
    })?;
    Ok(tokens
        .iter()
        .map(|token| ProcMacroToken::from_token(token, fallback_span))
        .collect())
}

fn token_kind(token: &Token) -> String {
    let debug = format!("{token:?}");
    debug.split(['(', ' ']).next().unwrap_or(debug.as_str()).to_string()
}

fn error_at_attr(attr: &Attribute, message: &str) -> ParseError {
    if let Some(span) = &attr.span {
        ParseError::with_span(message.to_string(), span.clone())
    } else {
        ParseError::new(message.to_string())
    }
}

fn error_from_span(span: Option<&Span>, message: &str) -> ParseError {
    if let Some(span) = span {
        ParseError::with_span(message.to_string(), span.clone())
    } else {
        ParseError::new(message.to_string())
    }
}

#[cfg(test)]
mod tests;
