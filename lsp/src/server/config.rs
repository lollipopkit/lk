use std::sync::Arc;

use serde::Deserialize;
use tokio::sync::Semaphore;
use tower_lsp::lsp_types::ConfigurationItem;

use super::state::LkrLanguageServer;

#[derive(Debug, Clone)]
pub(crate) struct ServerConfig {
    pub(crate) inlay_hints_enabled: bool,
    pub(crate) inlay_hints_parameters: bool,
    pub(crate) inlay_hints_types: bool,
    pub(crate) max_concurrent: usize,
    pub(crate) range_token_cache_limit: usize,
    pub(crate) inlay_hint_cache_limit: usize,
    pub(crate) inlay_scan_margin_lines: usize,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            inlay_hints_enabled: true,
            inlay_hints_parameters: true,
            inlay_hints_types: true,
            max_concurrent: 2,
            range_token_cache_limit: 64,
            inlay_hint_cache_limit: 64,
            inlay_scan_margin_lines: 3,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct LkrLspConfigSection {
    #[serde(default)]
    inlay_hints: InlayHintsConfig,
    #[serde(default)]
    performance: PerformanceConfig,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct InlayHintsConfig {
    #[serde(default)]
    enabled: Option<bool>,
    #[serde(default)]
    parameters: InlayKindConfig,
    #[serde(default)]
    types: InlayKindConfig,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct InlayKindConfig {
    enabled: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct PerformanceConfig {
    #[serde(default)]
    max_concurrent: Option<usize>,
    #[serde(default)]
    range_token_cache_limit: Option<usize>,
    #[serde(default)]
    inlay_hint_cache_limit: Option<usize>,
    #[serde(default)]
    inlay_scan_margin_lines: Option<usize>,
}

impl LkrLanguageServer {
    pub(crate) async fn load_config(&self) {
        let items = vec![ConfigurationItem {
            scope_uri: None,
            section: Some("lkr.lsp".to_string()),
        }];

        if let Ok(values) = self.client.configuration(items).await {
            if let Some(val) = values.into_iter().next() {
                if let Ok(cfg) = serde_json::from_value::<LkrLspConfigSection>(val) {
                    let mut guard = self.config.lock().unwrap();
                    guard.inlay_hints_enabled = cfg.inlay_hints.enabled.unwrap_or(true);
                    guard.inlay_hints_parameters = cfg.inlay_hints.parameters.enabled.unwrap_or(true);
                    guard.inlay_hints_types = cfg.inlay_hints.types.enabled.unwrap_or(true);

                    if let Some(v) = cfg.performance.max_concurrent.filter(|v| *v > 0) {
                        guard.max_concurrent = v;
                    }
                    if let Some(v) = cfg.performance.range_token_cache_limit.filter(|v| *v > 0) {
                        guard.range_token_cache_limit = v;
                    }
                    if let Some(v) = cfg.performance.inlay_hint_cache_limit.filter(|v| *v > 0) {
                        guard.inlay_hint_cache_limit = v;
                    }
                    if let Some(v) = cfg.performance.inlay_scan_margin_lines.filter(|v| *v > 0) {
                        guard.inlay_scan_margin_lines = v;
                    }

                    let permits = guard.max_concurrent.max(1);
                    if let Ok(mut sem_arc) = self.compute_limiter.lock() {
                        *sem_arc = Arc::new(Semaphore::new(permits));
                    }
                }
            }
        }
    }
}
