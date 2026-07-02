// Renders a demo MIR module to LLVM IR on stdout (used to smoke-test the
// MIR -> codegen -> clang -> run pipeline).
use lk_aot_mir::*;

fn main() {
    let (a, b, out) = (ValueId(0), ValueId(1), ValueId(2));
    let m = MirModule {
        abi_version: lk_aot_abi::ABI_VERSION,
        globals: vec![],
        entry: FuncId(0),
        functions: vec![MirFunction {
            id: FuncId(0),
            params: vec![],
            entry: BlockId(0),
            ret: Ty::I64,
            blocks: vec![Block {
                id: BlockId(0),
                params: vec![],
                insts: vec![
                    Inst::Const {
                        dst: a,
                        value: Const::I64(20),
                    },
                    Inst::Const {
                        dst: b,
                        value: Const::I64(4),
                    },
                    Inst::IntBin {
                        dst: out,
                        op: IntBinOp::Div,
                        lhs: a,
                        rhs: b,
                    },
                ],
                term: Term::Ret(Some(out)),
            }],
        }],
    };
    validate(&m).expect("valid");
    print!("{}", lk_aot_codegen::render_module(&m));
}
