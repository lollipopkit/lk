use super::expr_impl::Pattern;

impl core::fmt::Display for Pattern {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Pattern::Literal(val) => write!(f, "{}", val),
            Pattern::Variable(name) => write!(f, "{}", name),
            Pattern::Wildcard => write!(f, "_"),
            Pattern::List { patterns, rest } => {
                write!(f, "[")?;
                for (i, pattern) in patterns.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}", pattern)?;
                }
                if let Some(rest_name) = rest {
                    if !patterns.is_empty() {
                        write!(f, ", ")?;
                    }
                    write!(f, "..{}", rest_name)?;
                }
                write!(f, "]")
            }
            Pattern::Map { patterns, rest } => {
                write!(f, "{{")?;
                for (i, (key, pattern)) in patterns.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "\"{}\": {}", key, pattern)?;
                }
                if let Some(rest_name) = rest {
                    if !patterns.is_empty() {
                        write!(f, ", ")?;
                    }
                    write!(f, "..{}", rest_name)?;
                }
                write!(f, "}}")
            }
            Pattern::Or(patterns) => {
                for (i, pattern) in patterns.iter().enumerate() {
                    if i > 0 {
                        write!(f, " | ")?;
                    }
                    write!(f, "{}", pattern)?;
                }
                Ok(())
            }
            Pattern::Guard { pattern, guard } => {
                write!(f, "{} if {}", pattern, guard)
            }
            Pattern::Range { start, end, inclusive } => {
                let op = if *inclusive { "..=" } else { ".." };
                write!(f, "{}{}{}", start, op, end)
            }
        }
    }
}
