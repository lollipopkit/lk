use crate::val::RuntimeVal;

pub(super) enum ReturnValues32 {
    None,
    One(RuntimeVal),
    Two([RuntimeVal; 2]),
    Three([RuntimeVal; 3]),
    Four([RuntimeVal; 4]),
    Many(Vec<RuntimeVal>),
}

impl ReturnValues32 {
    pub(super) fn from_slice(values: &[RuntimeVal]) -> Self {
        match values {
            [] => Self::None,
            [one] => Self::One(one.clone()),
            [one, two] => Self::Two([one.clone(), two.clone()]),
            [one, two, three] => Self::Three([one.clone(), two.clone(), three.clone()]),
            [one, two, three, four] => Self::Four([one.clone(), two.clone(), three.clone(), four.clone()]),
            values => Self::Many(values.to_vec()),
        }
    }

    #[inline]
    pub(super) fn into_vec(self) -> Vec<RuntimeVal> {
        match self {
            Self::None => Vec::new(),
            Self::One(value) => vec![value],
            Self::Two(values) => Vec::from(values),
            Self::Three(values) => Vec::from(values),
            Self::Four(values) => Vec::from(values),
            Self::Many(values) => values,
        }
    }

    #[inline]
    pub(super) fn into_first(self) -> RuntimeVal {
        match self {
            Self::None => RuntimeVal::Nil,
            Self::One(value) => value,
            Self::Two([value, _]) => value,
            Self::Three([value, _, _]) => value,
            Self::Four([value, _, _, _]) => value,
            Self::Many(values) => values.into_iter().next().unwrap_or(RuntimeVal::Nil),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn return_values_inline_common_small_counts() {
        assert!(matches!(ReturnValues32::from_slice(&[]), ReturnValues32::None));
        assert!(matches!(
            ReturnValues32::from_slice(&[RuntimeVal::Int(1)]),
            ReturnValues32::One(RuntimeVal::Int(1))
        ));
        assert!(matches!(
            ReturnValues32::from_slice(&[RuntimeVal::Int(1), RuntimeVal::Int(2)]),
            ReturnValues32::Two([RuntimeVal::Int(1), RuntimeVal::Int(2)])
        ));
        assert!(matches!(
            ReturnValues32::from_slice(&[
                RuntimeVal::Int(1),
                RuntimeVal::Int(2),
                RuntimeVal::Int(3),
                RuntimeVal::Int(4)
            ]),
            ReturnValues32::Four([
                RuntimeVal::Int(1),
                RuntimeVal::Int(2),
                RuntimeVal::Int(3),
                RuntimeVal::Int(4)
            ])
        ));
        assert!(matches!(
            ReturnValues32::from_slice(&[
                RuntimeVal::Int(1),
                RuntimeVal::Int(2),
                RuntimeVal::Int(3),
                RuntimeVal::Int(4),
                RuntimeVal::Int(5)
            ]),
            ReturnValues32::Many(_)
        ));
    }
}
