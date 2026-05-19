use serde::Deserialize;
use tower_lsp::lsp_types::{
    InitializeParams, SemanticTokenModifier, SemanticTokenType, SemanticTokensFullOptions, SemanticTokensLegend,
    SemanticTokensOptions, SemanticTokensServerCapabilities,
};

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct ServerInitializationOptions {
    #[serde(default)]
    lk: LkInitializationOptions,
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct LkInitializationOptions {
    #[serde(default)]
    enable_semantic_tokens: bool,
}

pub(super) fn semantic_tokens_provider_from(params: &InitializeParams) -> Option<SemanticTokensServerCapabilities> {
    let options = params
        .initialization_options
        .as_ref()
        .and_then(|value| serde_json::from_value::<ServerInitializationOptions>(value.clone()).ok())
        .unwrap_or_default()
        .lk;

    if !options.enable_semantic_tokens {
        return None;
    }

    Some(SemanticTokensServerCapabilities::SemanticTokensOptions(
        SemanticTokensOptions {
            work_done_progress_options: Default::default(),
            legend: SemanticTokensLegend {
                token_types: vec![
                    SemanticTokenType::COMMENT,
                    SemanticTokenType::KEYWORD,
                    SemanticTokenType::VARIABLE,
                    SemanticTokenType::FUNCTION,
                    SemanticTokenType::STRING,
                    SemanticTokenType::NUMBER,
                    SemanticTokenType::OPERATOR,
                    SemanticTokenType::PARAMETER,
                    SemanticTokenType::PROPERTY,
                    SemanticTokenType::NAMESPACE,
                    SemanticTokenType::TYPE,
                ],
                token_modifiers: vec![
                    SemanticTokenModifier::DECLARATION,
                    SemanticTokenModifier::DEFINITION,
                    SemanticTokenModifier::READONLY,
                    SemanticTokenModifier::STATIC,
                ],
            },
            range: Some(true),
            full: Some(SemanticTokensFullOptions::Delta { delta: Some(true) }),
        },
    ))
}
