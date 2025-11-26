use crate::typ::{NumericClass, NumericHierarchy};

#[derive(Debug, Clone, PartialEq)]
pub struct FunctionNamedParamType {
    pub name: String,
    pub ty: Type,
    pub has_default: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Type {
    /// Primitive types
    Int,
    Float,
    String,
    Bool,
    Nil,

    /// Generic container types
    List(Box<Type>), // List<T>
    Map(Box<Type>, Box<Type>), // Map<K, V>
    /// Fixed-length heterogeneous tuple: (T0, T1, ...)
    Tuple(Vec<Type>),

    /// Function type with parameters and return type
    Function {
        params: Vec<Type>,
        named_params: Vec<FunctionNamedParamType>,
        return_type: Box<Type>,
    },

    /// Concurrency types
    Task(Box<Type>),
    Channel(Box<Type>),

    /// Union types: Int | String
    Union(Vec<Type>),

    /// Optional types: ?Int (sugar for Int | Nil)
    Optional(Box<Type>),

    /// Type variables for inference (prefixed with ')
    Variable(String),

    /// Custom named types
    Named(String),

    /// Generic type with parameters: List<T>, Map<K, V>
    Generic {
        name: String,
        params: Vec<Type>,
    },

    /// Boxed runtime value that preserves inner type metadata
    Boxed(Box<Type>),

    /// Any type (top type)
    Any,
}

impl Type {
    pub fn parse(s: &str) -> Option<Type> {
        let s = s.trim();

        // Handle primitive types
        match s {
            "Int" => return Some(Type::Int),
            "Float" => return Some(Type::Float),
            "String" => return Some(Type::String),
            "Bool" => return Some(Type::Bool),
            "Nil" => return Some(Type::Nil),
            "Any" => return Some(Type::Any),
            _ => {}
        }

        // Handle optional types: ?Int or Int?
        // Prefix form
        if let Some(inner) = s.strip_prefix('?') {
            return Type::parse(inner).map(|t| Type::Optional(Box::new(t)));
        }
        // Suffix form (allow trailing whitespace before '?')
        let s_no_ws = s.trim_end();
        if let Some(inner) = s_no_ws.strip_suffix('?') {
            let inner = inner.trim_end();
            if !inner.is_empty() {
                return Type::parse(inner).map(|t| Type::Optional(Box::new(t)));
            }
        }

        // Handle type variables: 'T, 'K, 'V
        if s.starts_with('\'') && s.len() > 1 {
            return Some(Type::Variable(s[1..].to_string()));
        }

        // Handle union types: Int | String | Nil
        if s.contains(" | ") {
            let types: Vec<Type> = s.split(" | ").filter_map(Type::parse).collect();
            if !types.is_empty() {
                return Some(Type::Union(types));
            }
        }

        // Handle generic types with angle brackets
        if let Some(open) = s.find('<')
            && let Some(close) = s.rfind('>')
        {
            let base = &s[..open];
            let params_str = &s[open + 1..close];

            // Parse type parameters
            let params: Vec<Type> = if params_str.is_empty() {
                vec![]
            } else {
                params_str.split(',').map(str::trim).filter_map(Type::parse).collect()
            };

            // Handle specific generic types
            match base {
                "List" => {
                    if params.len() == 1 {
                        return Some(Type::List(Box::new(params[0].clone())));
                    }
                }
                "Map" => {
                    if params.len() == 2 {
                        return Some(Type::Map(Box::new(params[0].clone()), Box::new(params[1].clone())));
                    }
                }
                "Task" => {
                    if params.len() == 1 {
                        return Some(Type::Task(Box::new(params[0].clone())));
                    }
                }
                "Channel" => {
                    if params.len() == 1 {
                        return Some(Type::Channel(Box::new(params[0].clone())));
                    }
                }
                "Box" | "Boxed" => {
                    if params.len() == 1 {
                        return Some(Type::Boxed(Box::new(params[0].clone())));
                    }
                }
                _ => {
                    // Generic custom type
                    return Some(Type::Generic {
                        name: base.to_string(),
                        params,
                    });
                }
            }
        }

        // Handle function types: (Int, String) -> Bool
        if s.contains("->") {
            let parts: Vec<&str> = s.splitn(2, "->").collect();
            if parts.len() == 2 {
                let params_str = parts[0].trim();
                let return_str = parts[1].trim();

                // Parse parameters
                let params = if params_str.starts_with('(') && params_str.ends_with(')') {
                    let inner = &params_str[1..params_str.len() - 1];
                    if inner.is_empty() {
                        vec![]
                    } else {
                        inner.split(',').map(str::trim).filter_map(Type::parse).collect()
                    }
                } else {
                    vec![]
                };

                // Parse return type
                if let Some(return_type) = Type::parse(return_str) {
                    return Some(Type::Function {
                        params,
                        named_params: Vec::new(),
                        return_type: Box::new(return_type),
                    });
                }
            }
        }

        // Handle bare List and Map as generic types
        match s {
            "List" => Some(Type::List(Box::new(Type::Any))),
            "Map" => Some(Type::Map(Box::new(Type::Any), Box::new(Type::Any))),
            _ => {
                // Assume it's a named custom type
                if s.chars().all(|c| c.is_alphanumeric() || c == '_') {
                    Some(Type::Named(s.to_string()))
                } else {
                    None
                }
            }
        }
    }

    /// Get a display representation of the type
    pub fn display(&self) -> String {
        match self {
            Type::Int => "Int".to_string(),
            Type::Float => "Float".to_string(),
            Type::String => "String".to_string(),
            Type::Bool => "Bool".to_string(),
            Type::Nil => "Nil".to_string(),
            Type::Any => "Any".to_string(),
            Type::List(elem) => format!("List<{}>", elem.display()),
            Type::Map(k, v) => format!("Map<{}, {}>", k.display(), v.display()),
            Type::Tuple(elems) => {
                if elems.is_empty() {
                    "Tuple<>".to_string()
                } else {
                    let parts: Vec<String> = elems.iter().map(|t| t.display()).collect();
                    format!("Tuple<{}>", parts.join(", "))
                }
            }
            Type::Function {
                params,
                named_params,
                return_type,
            } => {
                let mut segments: Vec<String> = Vec::new();
                if !params.is_empty() {
                    segments.extend(params.iter().map(|p| p.display()));
                }
                if !named_params.is_empty() {
                    let named_parts: Vec<String> = named_params
                        .iter()
                        .map(|np| {
                            let mut s = format!("{}: {}", np.name, np.ty.display());
                            if np.has_default {
                                s.push_str(" = _");
                            }
                            s
                        })
                        .collect();
                    segments.push(format!("{{{}}}", named_parts.join(", ")));
                }
                format!("({}) -> {}", segments.join(", "), return_type.display())
            }
            Type::Task(inner) => format!("Task<{}>", inner.display()),
            Type::Channel(inner) => format!("Channel<{}>", inner.display()),
            Type::Union(types) => {
                let type_strs: Vec<String> = types.iter().map(|t| t.display()).collect();
                type_strs.join(" | ")
            }
            Type::Optional(inner) => format!("{}?", inner.display()),
            Type::Variable(name) => format!("'{}", name),
            Type::Named(name) => name.clone(),
            Type::Generic { name, params } => {
                if params.is_empty() {
                    name.clone()
                } else {
                    let param_strs: Vec<String> = params.iter().map(|p| p.display()).collect();
                    format!("{}<{}>", name, param_strs.join(", "))
                }
            }
            Type::Boxed(inner) => format!("Box<{}>", inner.display()),
        }
    }

    /// Check if this type can be assigned to another type (subtyping)
    pub fn is_assignable_to(&self, other: &Type) -> bool {
        match (self, other) {
            // Any type is assignable to Any
            (_, Type::Any) => true,
            // Any can flow into any type (dynamic fallback)
            (Type::Any, _) => true,
            // Same types are assignable
            (a, b) if a == b => true,
            // Numeric hierarchy: allow Int -> Float, Float -> Boxed, etc.
            (lhs, rhs) if lhs.numeric_class().is_some() && rhs.numeric_class().is_some() => {
                let lhs_class = lhs.numeric_class().unwrap();
                let rhs_class = rhs.numeric_class().unwrap();
                lhs_class <= rhs_class
            }
            // Optional types: T is assignable to ?T
            (inner, Type::Optional(expected_inner)) => inner.is_assignable_to(expected_inner),
            // Union types: T is assignable to Union if T is assignable to any member
            (t, Type::Union(union_types)) => union_types.iter().any(|ut| t.is_assignable_to(ut)),
            // Union member is assignable to union
            (Type::Union(union_types), target) => union_types.iter().all(|ut| ut.is_assignable_to(target)),
            // Boxed types act as transparent wrappers
            (Type::Boxed(inner), Type::Boxed(expected)) => inner.is_assignable_to(expected),
            (Type::Boxed(inner), expected) => inner.is_assignable_to(expected),
            (actual, Type::Boxed(expected)) => actual.is_assignable_to(expected),
            // Generic containers with covariant element types
            (Type::List(a), Type::List(b)) => a.is_assignable_to(b),
            (Type::Map(ak, av), Type::Map(bk, bv)) => ak.is_assignable_to(bk) && av.is_assignable_to(bv),
            (Type::Tuple(as_), Type::Tuple(bs)) => {
                as_.len() == bs.len() && as_.iter().zip(bs.iter()).all(|(a, b)| a.is_assignable_to(b))
            }
            // Function types (contravariant parameters, covariant return)
            (
                Type::Function {
                    params: a_params,
                    named_params: a_named,
                    return_type: a_ret,
                },
                Type::Function {
                    params: b_params,
                    named_params: b_named,
                    return_type: b_ret,
                },
            ) => {
                if a_params.len() != b_params.len() {
                    false
                } else {
                    // Parameters are contravariant
                    let params_compatible = b_params
                        .iter()
                        .zip(a_params.iter())
                        .all(|(b_param, a_param)| b_param.is_assignable_to(a_param));
                    if !params_compatible {
                        return false;
                    }

                    if a_named.len() != b_named.len() {
                        return false;
                    }
                    let a_map: std::collections::HashMap<&str, &FunctionNamedParamType> =
                        a_named.iter().map(|np| (np.name.as_str(), np)).collect();
                    let named_compatible = b_named.iter().all(|b_np| {
                        if let Some(a_np) = a_map.get(b_np.name.as_str()) {
                            b_np.has_default == a_np.has_default && b_np.ty.is_assignable_to(&a_np.ty)
                        } else {
                            false
                        }
                    });
                    if !named_compatible {
                        return false;
                    }
                    // Return type is covariant
                    let return_compatible = a_ret.is_assignable_to(b_ret);
                    params_compatible && named_compatible && return_compatible
                }
            }
            // Concurrency types
            (Type::Task(a), Type::Task(b)) => a.is_assignable_to(b),
            (Type::Channel(a), Type::Channel(b)) => a.is_assignable_to(b),
            // No other assignability rules
            _ => false,
        }
    }

    /// Map type into numeric hierarchy class when applicable.
    pub fn numeric_class(&self) -> Option<NumericClass> {
        NumericHierarchy::classify(self)
    }

    /// Check if this type contains any type variables
    pub fn contains_variables(&self) -> bool {
        match self {
            Type::Variable(_) => true,
            Type::List(inner) | Type::Optional(inner) | Type::Task(inner) | Type::Channel(inner) => {
                inner.contains_variables()
            }
            Type::Map(k, v) => k.contains_variables() || v.contains_variables(),
            Type::Function {
                params,
                named_params,
                return_type,
            } => {
                params.iter().any(|p| p.contains_variables())
                    || named_params.iter().any(|np| np.ty.contains_variables())
                    || return_type.contains_variables()
            }
            Type::Union(types) => types.iter().any(|t| t.contains_variables()),
            Type::Tuple(elems) => elems.iter().any(|t| t.contains_variables()),
            Type::Generic { params, .. } => params.iter().any(|p| p.contains_variables()),
            Type::Boxed(inner) => inner.contains_variables(),
            _ => false,
        }
    }

    /// Substitute type variables with concrete types
    pub fn substitute(&self, substitutions: &std::collections::HashMap<String, Type>) -> Type {
        match self {
            Type::Variable(name) => substitutions.get(name).cloned().unwrap_or_else(|| self.clone()),
            Type::List(inner) => Type::List(Box::new(inner.substitute(substitutions))),
            Type::Map(k, v) => Type::Map(
                Box::new(k.substitute(substitutions)),
                Box::new(v.substitute(substitutions)),
            ),
            Type::Function {
                params,
                named_params,
                return_type,
            } => Type::Function {
                params: params.iter().map(|p| p.substitute(substitutions)).collect(),
                named_params: named_params
                    .iter()
                    .map(|np| FunctionNamedParamType {
                        name: np.name.clone(),
                        ty: np.ty.substitute(substitutions),
                        has_default: np.has_default,
                    })
                    .collect(),
                return_type: Box::new(return_type.substitute(substitutions)),
            },
            Type::Tuple(elems) => Type::Tuple(elems.iter().map(|e| e.substitute(substitutions)).collect()),
            Type::Optional(inner) => Type::Optional(Box::new(inner.substitute(substitutions))),
            Type::Task(inner) => Type::Task(Box::new(inner.substitute(substitutions))),
            Type::Channel(inner) => Type::Channel(Box::new(inner.substitute(substitutions))),
            Type::Union(types) => Type::Union(types.iter().map(|t| t.substitute(substitutions)).collect()),
            Type::Generic { name, params } => Type::Generic {
                name: name.clone(),
                params: params.iter().map(|p| p.substitute(substitutions)).collect(),
            },
            Type::Boxed(inner) => Type::Boxed(Box::new(inner.substitute(substitutions))),
            _ => self.clone(),
        }
    }
}
