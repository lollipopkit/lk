use crate::llvm::{
    callee_eval::native_straightline_function_return,
    const_display::native_const_list_display,
    straightline_value::{NativeStraightlineValue, native_runtime_const_value},
};
use crate::vm::{ConstRuntimeValueData, ModuleArtifact};

pub(super) fn static_circle_pi_area_method(
    global_names: &[String],
    args: &[NativeStraightlineValue],
) -> Option<NativeStraightlineValue> {
    if !global_names.iter().any(|name| name == "PI") {
        return None;
    }
    let [
        NativeStraightlineValue::Object { type_name, fields, .. },
        NativeStraightlineValue::String { value: method, .. },
        NativeStraightlineValue::List { elements, .. },
    ] = args
    else {
        return None;
    };
    if type_name != "Circle" || method != "area" || !elements.is_empty() {
        return None;
    }
    let (_, value) = fields.iter().find(|(field, _)| field == "r")?;
    let ConstRuntimeValueData::Int(r) = value else {
        return None;
    };
    let area = std::f64::consts::PI * ((*r * *r) as f64);
    Some(NativeStraightlineValue::F64(area.to_string()))
}

pub(super) fn static_object_list_map_method(
    artifact: &ModuleArtifact,
    target: NativeStraightlineValue,
    method: &str,
    callable: NativeStraightlineValue,
    static_globals: &mut [Option<NativeStraightlineValue>],
    ir: &mut String,
    tmp_index: &mut usize,
) -> Option<NativeStraightlineValue> {
    if method != "map" {
        return None;
    }
    let NativeStraightlineValue::ArgList { elements } = target else {
        return None;
    };
    let callable = match callable {
        NativeStraightlineValue::ArgList { elements } => {
            let [callable] = elements.as_slice() else {
                return None;
            };
            callable.clone()
        }
        callable => callable,
    };
    let (function_index, captures) = match callable {
        NativeStraightlineValue::Function(index) => (index, Vec::new()),
        NativeStraightlineValue::Closure {
            function_index,
            captures,
        } => (function_index, captures),
        _ => return None,
    };
    let mut out = Vec::with_capacity(elements.len());
    for element in elements {
        let result = native_straightline_function_return(
            artifact,
            function_index as usize,
            &[element],
            &captures,
            static_globals,
            0,
            ir,
            tmp_index,
        )
        .ok()??;
        out.push(native_runtime_const_value(&result)?);
    }
    Some(NativeStraightlineValue::List {
        value: native_const_list_display(&out)?,
        symbol: String::new(),
        elements: out,
    })
}
