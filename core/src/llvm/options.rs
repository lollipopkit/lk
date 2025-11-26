use std::fmt;

/// Optimization level to feed into LLVM's `opt` tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OptLevel {
    None,
    O1,
    #[default]
    O2,
    O3,
}

impl OptLevel {
    pub fn as_flag(&self) -> &'static str {
        match self {
            OptLevel::None => "-O0",
            OptLevel::O1 => "-O1",
            OptLevel::O2 => "-O2",
            OptLevel::O3 => "-O3",
        }
    }
}

impl fmt::Display for OptLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OptLevel::None => write!(f, "O0"),
            OptLevel::O1 => write!(f, "O1"),
            OptLevel::O2 => write!(f, "O2"),
            OptLevel::O3 => write!(f, "O3"),
        }
    }
}

/// Configures behaviour of the LLVM backend.
#[derive(Debug, Clone)]
pub struct LlvmBackendOptions {
    /// Module name emitted in the IR header.
    pub module_name: String,
    /// Target triple to record in the module (if provided).
    pub target_triple: Option<String>,
    /// Whether to run LLVM optimisation passes via `opt`.
    pub run_optimizations: bool,
    /// Optimisation level when [`LlvmBackendOptions::run_optimizations`] is true.
    pub opt_level: OptLevel,
}

impl Default for LlvmBackendOptions {
    fn default() -> Self {
        Self {
            module_name: "lkr_module".to_string(),
            target_triple: None,
            run_optimizations: true,
            opt_level: OptLevel::default(),
        }
    }
}
