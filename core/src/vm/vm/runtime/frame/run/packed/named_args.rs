use anyhow::{Result, anyhow};

use crate::val::Val;
use crate::vm::copy_call_arg_value_for_register_with_metrics;

pub(super) fn load_named_pairs(
    regs: &[Val],
    named_start: usize,
    named_len: usize,
    out: &mut Vec<(String, Val)>,
    collect_metrics: bool,
) -> Result<()> {
    for i in 0..named_len {
        let key_val = &regs[named_start + 2 * i];
        let val = copy_call_arg_value_for_register_with_metrics(&regs[named_start + 2 * i + 1], collect_metrics);
        let key = match key_val {
            Val::Str(s) => s.to_string(),
            Val::ShortStr(s) => s.as_str().to_string(),
            Val::Int(i) => i.to_string(),
            Val::Float(f) => f.to_string(),
            Val::Bool(b) => b.to_string(),
            _ => return Err(anyhow!("Named argument key must be primitive, got {:?}", key_val)),
        };
        out.push((key, val));
    }
    Ok(())
}
