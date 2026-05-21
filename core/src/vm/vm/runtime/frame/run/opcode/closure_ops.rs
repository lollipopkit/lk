use super::*;
use crate::vm::bytecode::Function;

#[allow(clippy::too_many_arguments)]
pub(super) fn run_make_closure_opcode(
    regs: &mut [Val],
    ctx: &mut VmContext,
    pc_ref: &mut usize,
    f: &Function,
    dst: &u16,
    proto: &u16,
    collect_metrics: bool,
) -> Result<Option<Val>> {
    let mut pc = *pc_ref;
    let p = f
        .protos
        .get(*proto as usize)
        .ok_or_else(|| anyhow!("closure proto out of range"))?;
    if p.self_name.is_none() && p.captures.is_empty() {
        let clo = copy_value_for_register_with_metrics(
            p.empty_closure.get_or_init(|| {
                let clo = Val::Closure(Arc::new(ClosureValue::new(ClosureInit {
                    params: Arc::clone(&p.params),
                    param_types: Arc::clone(&p.param_types),
                    named_params: Arc::clone(&p.named_params),
                    body: Arc::clone(&p.body),
                    env: Arc::clone(&p.empty_env),
                    upvalues: Arc::clone(&p.empty_upvalues),
                    captures: Arc::clone(&p.empty_captures),
                    capture_specs: Arc::clone(&p.captures),
                    default_funcs: Arc::clone(&p.default_funcs),
                    code: Arc::clone(&p.code),
                    debug_name: None,
                    debug_location: None,
                })));
                if p.func.is_none()
                    && p.code.get().is_none()
                    && let Val::Closure(closure_arc) = &clo
                {
                    let c = Compiler::new();
                    let compiled = c.compile_function_with_param_types_and_captures(
                        p.params.as_ref(),
                        p.param_types.as_ref(),
                        p.named_params.as_ref(),
                        p.body.as_ref(),
                        p.captures.as_ref(),
                    );
                    let _ = closure_arc.code.set(Arc::new(compiled));
                }
                clo
            }),
            collect_metrics,
        );
        assign_reg_with_metrics(regs, *dst as usize, clo, collect_metrics);
        pc += 1;
        *pc_ref = pc;
        return Ok(None);
    }
    let captured_env = if p.self_name.is_some() {
        Arc::new(ctx.snapshot())
    } else {
        Arc::clone(&p.empty_env)
    };

    let captures = if p.captures.is_empty() {
        Arc::clone(&p.empty_captures)
    } else if let [spec] = p.captures.as_ref().as_slice() {
        let value = copy_capture_spec_value(ctx, regs, &f.consts, spec, collect_metrics);
        ClosureCapture::from_shared_names_one(Arc::clone(&p.capture_names), value)
    } else {
        let mut values: Vec<Val> = Vec::with_capacity(p.captures.len());
        for spec in p.captures.iter() {
            values.push(copy_capture_spec_value(ctx, regs, &f.consts, spec, collect_metrics));
        }
        ClosureCapture::from_shared_names(Arc::clone(&p.capture_names), values)
    };

    let mut clo = Val::Closure(Arc::new(ClosureValue::new(ClosureInit {
        params: Arc::clone(&p.params),
        param_types: Arc::clone(&p.param_types),
        named_params: Arc::clone(&p.named_params),
        body: Arc::clone(&p.body),
        env: captured_env,
        upvalues: Arc::clone(&p.empty_upvalues),
        captures,
        capture_specs: Arc::clone(&p.captures),
        default_funcs: Arc::clone(&p.default_funcs),
        code: Arc::clone(&p.code),
        debug_name: p.self_name.clone(),
        debug_location: None,
    })));
    let self_binding = p.self_name.as_ref().map(|name| {
        (
            name.clone(),
            copy_value_for_register_with_metrics(&clo, collect_metrics),
        )
    });
    if let (Some((name, clone_for_env)), Val::Closure(closure_arc)) = (self_binding, &mut clo)
        && let Some(closure) = Arc::get_mut(closure_arc)
        && let Some(env_mut) = Arc::get_mut(&mut closure.env)
    {
        env_mut.define(name, clone_for_env);
    }
    if p.func.is_none()
        && p.code.get().is_none()
        && let Val::Closure(closure_arc) = &clo
    {
        // Eagerly pre-compile closures to eliminate OnceCell overhead from hot calls
        let c = Compiler::new();
        let compiled = c.compile_function_with_param_types_and_captures(
            p.params.as_ref(),
            p.param_types.as_ref(),
            p.named_params.as_ref(),
            p.body.as_ref(),
            p.captures.as_ref(),
        );
        let _ = closure_arc.code.set(Arc::new(compiled));
    }
    assign_reg_with_metrics(regs, *dst as usize, clo, collect_metrics);
    pc += 1;
    *pc_ref = pc;
    Ok(None)
}
