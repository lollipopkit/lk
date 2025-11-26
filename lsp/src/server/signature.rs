use tower_lsp::lsp_types::{Documentation, ParameterInformation, ParameterLabel, SignatureInformation};

pub(crate) fn sig(label: &str, params: &[&str], doc: &str) -> SignatureInformation {
    SignatureInformation {
        label: label.to_string(),
        documentation: Some(Documentation::String(doc.to_string())),
        parameters: Some(
            params
                .iter()
                .map(|p| ParameterInformation {
                    label: ParameterLabel::Simple((*p).to_string()),
                    documentation: None,
                })
                .collect(),
        ),
        active_parameter: None,
    }
}

pub(crate) fn sig_owned(label: String, params: Vec<String>, doc: &str) -> SignatureInformation {
    SignatureInformation {
        label,
        documentation: Some(Documentation::String(doc.to_string())),
        parameters: Some(
            params
                .into_iter()
                .map(|p| ParameterInformation {
                    label: ParameterLabel::Simple(p),
                    documentation: None,
                })
                .collect(),
        ),
        active_parameter: None,
    }
}
