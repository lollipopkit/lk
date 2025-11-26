use crate::{
    stmt::{Stmt, StmtParser},
    token::Tokenizer,
    val::Val,
    vm::{Compiler, Function, Vm, VmContext},
};
use anyhow::{Context, Result};

pub struct VmRunOutcome {
    pub value: Val,
    pub heap_bytes: u64,
}

#[derive(Clone)]
pub struct PreparedScriptScenario {
    spec: &'static ScriptScenario,
    function: Function,
}

impl PreparedScriptScenario {
    pub fn key(&self) -> &'static str {
        self.spec.key
    }

    pub fn title(&self) -> &'static str {
        self.spec.title
    }

    pub fn bench_case_name(&self) -> String {
        format!("{}_vm", self.spec.key)
    }

    pub fn run_vm(&self) -> Result<VmRunOutcome> {
        let mut vm = Vm::new();
        self.run_with_vm(&mut vm)
    }

    pub fn run_with_vm(&self, vm: &mut Vm) -> Result<VmRunOutcome> {
        let mut ctx = VmContext::new();
        let value = vm
            .exec_with(&self.function, &mut ctx, None)
            .context("vm execution failed for script scenario")?;
        self.spec.expected.verify(&value)?;
        Ok(VmRunOutcome {
            value,
            heap_bytes: vm.heap_bytes(),
        })
    }
}

#[derive(Clone)]
struct ScriptScenario {
    key: &'static str,
    title: &'static str,
    script: &'static str,
    expected: ExpectedValue,
}

#[derive(Clone)]
#[allow(dead_code)]
enum ExpectedValue {
    Int(i64),
    Float(f64),
    Bool(bool),
    Nil,
}

impl ExpectedValue {
    fn verify(&self, actual: &Val) -> Result<()> {
        let matches = match self {
            ExpectedValue::Int(v) => actual == &Val::Int(*v),
            ExpectedValue::Float(v) => match actual {
                Val::Float(f) => (f - v).abs() <= f64::EPSILON,
                _ => false,
            },
            ExpectedValue::Bool(v) => actual == &Val::Bool(*v),
            ExpectedValue::Nil => actual == &Val::Nil,
        };
        if matches {
            Ok(())
        } else {
            Err(anyhow::anyhow!("expected {:?} but observed {:?}", self, actual))
        }
    }
}

impl std::fmt::Debug for ExpectedValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExpectedValue::Int(v) => write!(f, "Int({})", v),
            ExpectedValue::Float(v) => write!(f, "Float({})", v),
            ExpectedValue::Bool(v) => write!(f, "Bool({})", v),
            ExpectedValue::Nil => write!(f, "Nil"),
        }
    }
}

const FIB_SCRIPT: &str = concat!(include_str!("../../../examples/fib.lkr"), "\nreturn iterative(30);\n");

const REPL_SEQUENCE_SCRIPT: &str = r#"
let total = 0;
let i = 0;
while (i < 100) {
    total = total + i;
    i = i + 1;
}
return total;
"#;

const NUMERIC_REDUCTION_SCRIPT: &str = r#"
let total = 0;
let i = 0;
while (i < 200) {
    let step = i + 1;
    total = total + step * (step + 1);
    i = i + 1;
}
return total;
"#;

static SCRIPT_SCENARIOS: &[ScriptScenario] = &[
    ScriptScenario {
        key: "script_fib",
        title: "Fibonacci VM baseline",
        script: FIB_SCRIPT,
        expected: ExpectedValue::Int(832_040),
    },
    ScriptScenario {
        key: "repl_sequence",
        title: "REPL-style arithmetic loop",
        script: REPL_SEQUENCE_SCRIPT,
        expected: ExpectedValue::Int(4_950),
    },
    ScriptScenario {
        key: "numeric_reduction",
        title: "Nested arithmetic reduction loop",
        script: NUMERIC_REDUCTION_SCRIPT,
        expected: ExpectedValue::Int(2_706_800),
    },
];

fn parse_block(source: &str) -> Result<Stmt> {
    let (tokens, spans) = Tokenizer::tokenize_enhanced_with_spans(source).context("tokenize benchmark script")?;
    let mut parser = StmtParser::new_with_spans(&tokens, &spans);
    let program = parser
        .parse_program_with_enhanced_errors(source)
        .context("parse benchmark script")?;
    Ok(Stmt::Block {
        statements: program.statements,
    })
}

fn compile_block(block: &Stmt) -> Result<Function> {
    Ok(Compiler::new().compile_stmt(block))
}

pub fn prepare_script_scenarios() -> Result<Vec<PreparedScriptScenario>> {
    SCRIPT_SCENARIOS
        .iter()
        .map(|spec| {
            let block = parse_block(spec.script)?;
            let function = compile_block(&block)?;
            Ok(PreparedScriptScenario { spec, function })
        })
        .collect()
}
