use crate::val::RuntimeVal;

pub(super) enum ReturnValues {
    None,
    One(RuntimeVal),
    Two([RuntimeVal; 2]),
    Three([RuntimeVal; 3]),
    Four([RuntimeVal; 4]),
    Many(Vec<RuntimeVal>),
}

impl ReturnValues {
    pub(super) fn take_from_slots(values: &mut [RuntimeVal]) -> Self {
        match values {
            [] => Self::None,
            [one] => Self::One(std::mem::take(one)),
            [one, two] => Self::Two([std::mem::take(one), std::mem::take(two)]),
            [one, two, three] => Self::Three([std::mem::take(one), std::mem::take(two), std::mem::take(three)]),
            [one, two, three, four] => Self::Four([
                std::mem::take(one),
                std::mem::take(two),
                std::mem::take(three),
                std::mem::take(four),
            ]),
            values => {
                let mut out = Vec::with_capacity(values.len());
                for value in values {
                    out.push(std::mem::take(value));
                }
                Self::Many(out)
            }
        }
    }

    #[inline]
    pub(super) fn into_vec(self) -> Vec<RuntimeVal> {
        match self {
            Self::None => vec![RuntimeVal::Nil],
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
        let mut none = [];
        assert!(matches!(ReturnValues::take_from_slots(&mut none), ReturnValues::None));

        let mut one = [RuntimeVal::Int(1)];
        assert!(matches!(
            ReturnValues::take_from_slots(&mut one),
            ReturnValues::One(RuntimeVal::Int(1))
        ));
        assert_eq!(one, [RuntimeVal::Nil]);

        let mut two = [RuntimeVal::Int(1), RuntimeVal::Int(2)];
        assert!(matches!(
            ReturnValues::take_from_slots(&mut two),
            ReturnValues::Two([RuntimeVal::Int(1), RuntimeVal::Int(2)])
        ));
        assert_eq!(two, [RuntimeVal::Nil, RuntimeVal::Nil]);

        let mut four = [
            RuntimeVal::Int(1),
            RuntimeVal::Int(2),
            RuntimeVal::Int(3),
            RuntimeVal::Int(4),
        ];
        assert!(matches!(
            ReturnValues::take_from_slots(&mut four),
            ReturnValues::Four([
                RuntimeVal::Int(1),
                RuntimeVal::Int(2),
                RuntimeVal::Int(3),
                RuntimeVal::Int(4)
            ])
        ));
        assert_eq!(
            four,
            [RuntimeVal::Nil, RuntimeVal::Nil, RuntimeVal::Nil, RuntimeVal::Nil]
        );

        let mut many = [
            RuntimeVal::Int(1),
            RuntimeVal::Int(2),
            RuntimeVal::Int(3),
            RuntimeVal::Int(4),
            RuntimeVal::Int(5),
        ];
        assert!(matches!(
            ReturnValues::take_from_slots(&mut many),
            ReturnValues::Many(_)
        ));
        assert!(many.iter().all(|value| matches!(value, RuntimeVal::Nil)));
    }
}
