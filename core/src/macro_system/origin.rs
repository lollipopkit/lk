#[cfg(not(feature = "std"))]
use crate::compat::prelude::*;
use crate::token::Span;

use super::SourceToken;

#[derive(Debug, Clone, PartialEq)]
pub enum MacroOriginKind {
    CallSite,
    Definition,
    CrateAnchor,
    ProcMacroOutput,
}

impl MacroOriginKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::CallSite => "call_site",
            Self::Definition => "definition",
            Self::CrateAnchor => "crate_anchor",
            Self::ProcMacroOutput => "proc_macro_output",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct MacroOriginFrame {
    pub macro_name: String,
    pub call_span: Span,
    pub kind: MacroOriginKind,
}

impl MacroOriginFrame {
    pub fn new(macro_name: impl Into<String>, call_span: Span, kind: MacroOriginKind) -> Self {
        Self {
            macro_name: macro_name.into(),
            call_span,
            kind,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct MacroTokenOrigin {
    pub token_index: usize,
    pub lexeme: String,
    pub span: Span,
    pub frames: Vec<MacroOriginFrame>,
}

pub(in crate::macro_system) fn inherit_call_origin(token: &mut SourceToken, call_origins: &[MacroOriginFrame]) {
    token.origins = call_origins.to_vec();
}

pub(in crate::macro_system) fn push_origin(
    token: &mut SourceToken,
    macro_name: &str,
    call_span: &Span,
    kind: MacroOriginKind,
) {
    token
        .origins
        .push(MacroOriginFrame::new(macro_name, call_span.clone(), kind));
}
