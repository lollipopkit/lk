use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{Result, anyhow};
use lkr_core::{
    module::{Module, ModuleRegistry},
    val::{Val, methods::register_method},
    vm::VmContext,
};

#[derive(Debug)]
pub struct StringModule {
    functions: HashMap<String, Val>,
}

impl Default for StringModule {
    fn default() -> Self {
        Self::new()
    }
}

impl StringModule {
    pub fn new() -> Self {
        let mut functions = HashMap::new();

        // Register string functions as Rust functions
        functions.insert("len".to_string(), Val::RustFunction(Self::len));
        functions.insert("lower".to_string(), Val::RustFunction(Self::lower));
        functions.insert("upper".to_string(), Val::RustFunction(Self::upper));
        functions.insert("trim".to_string(), Val::RustFunction(Self::trim));
        functions.insert("starts_with".to_string(), Val::RustFunction(Self::starts_with));
        functions.insert("ends_with".to_string(), Val::RustFunction(Self::ends_with));
        functions.insert("contains".to_string(), Val::RustFunction(Self::contains));
        functions.insert("replace".to_string(), Val::RustFunctionNamed(Self::replace));
        functions.insert("substring".to_string(), Val::RustFunction(Self::substring));
        functions.insert("split".to_string(), Val::RustFunction(Self::split));
        functions.insert("join".to_string(), Val::RustFunction(Self::join));

        // Also register as meta-methods for String type
        register_method("String", "len", Self::len);
        register_method("String", "lower", Self::lower);
        register_method("String", "upper", Self::upper);
        register_method("String", "trim", Self::trim);
        register_method("String", "starts_with", Self::starts_with);
        register_method("String", "ends_with", Self::ends_with);
        register_method("String", "contains", Self::contains);
        register_method("String", "replace", Self::replace_method);
        register_method("String", "substring", Self::substring);
        register_method("String", "split", Self::split);
        register_method("String", "join", Self::join);

        Self { functions }
    }

    /// Get string length
    fn len(args: &[Val], _ctx: &mut VmContext) -> Result<Val> {
        if args.len() != 1 {
            return Err(anyhow!("len() takes exactly 1 argument"));
        }

        match &args[0] {
            Val::Str(s) => Ok(Val::Int(s.len() as i64)),
            _ => Err(anyhow!("len() argument must be a string")),
        }
    }

    /// Convert to lowercase
    fn lower(args: &[Val], _ctx: &mut VmContext) -> Result<Val> {
        if args.len() != 1 {
            return Err(anyhow!("lower() takes exactly 1 argument"));
        }

        match &args[0] {
            Val::Str(s) => Ok(Val::Str(s.to_lowercase().into())),
            _ => Err(anyhow!("lower() argument must be a string")),
        }
    }

    /// Convert to uppercase
    fn upper(args: &[Val], _ctx: &mut VmContext) -> Result<Val> {
        if args.len() != 1 {
            return Err(anyhow!("upper() takes exactly 1 argument"));
        }

        match &args[0] {
            Val::Str(s) => Ok(Val::Str(s.to_uppercase().into())),
            _ => Err(anyhow!("upper() argument must be a string")),
        }
    }

    /// Trim whitespace from both ends
    fn trim(args: &[Val], _ctx: &mut VmContext) -> Result<Val> {
        if args.len() != 1 {
            return Err(anyhow!("trim() takes exactly 1 argument"));
        }

        match &args[0] {
            Val::Str(s) => Ok(Val::Str(s.trim().into())),
            _ => Err(anyhow!("trim() argument must be a string")),
        }
    }

    /// Check if string starts with prefix
    fn starts_with(args: &[Val], _ctx: &mut VmContext) -> Result<Val> {
        if args.len() != 2 {
            return Err(anyhow!("starts_with() takes exactly 2 arguments: string, prefix"));
        }

        let string = match &args[0] {
            Val::Str(s) => &**s,
            _ => {
                return Err(anyhow!("starts_with() first argument must be a string"));
            }
        };

        let prefix = match &args[1] {
            Val::Str(p) => &**p,
            _ => {
                return Err(anyhow!("starts_with() second argument must be a string"));
            }
        };

        Ok(Val::Bool(string.starts_with(prefix)))
    }

    /// Check if string ends with suffix
    fn ends_with(args: &[Val], _ctx: &mut VmContext) -> Result<Val> {
        if args.len() != 2 {
            return Err(anyhow!("ends_with() takes exactly 2 arguments: string, suffix"));
        }

        let string = match &args[0] {
            Val::Str(s) => &**s,
            _ => {
                return Err(anyhow!("ends_with() first argument must be a string"));
            }
        };

        let suffix = match &args[1] {
            Val::Str(s) => &**s,
            _ => {
                return Err(anyhow!("ends_with() second argument must be a string"));
            }
        };

        Ok(Val::Bool(string.ends_with(suffix)))
    }

    /// Check if string contains substring
    fn contains(args: &[Val], _ctx: &mut VmContext) -> Result<Val> {
        if args.len() != 2 {
            return Err(anyhow!("contains() takes exactly 2 arguments: string, substring"));
        }

        let string = match &args[0] {
            Val::Str(s) => &**s,
            _ => {
                return Err(anyhow!("contains() first argument must be a string"));
            }
        };

        let substring = match &args[1] {
            Val::Str(s) => &**s,
            _ => {
                return Err(anyhow!("contains() second argument must be a string"));
            }
        };

        Ok(Val::Bool(string.contains(substring)))
    }

    /// Replace occurrences of substring with support for named parameters.
    /// Usage examples:
    /// - string.replace("foo", "o", "a")                // legacy positional API (replaces all)
    /// - string.replace("foo", pattern: "o", with: "a") // named API (defaults to first occurrence)
    /// - string.replace("foo", pattern: "o", with: "a", all: true)
    fn replace(pos: &[Val], named: &[(String, Val)], _ctx: &mut VmContext) -> Result<Val> {
        if pos.is_empty() {
            return Err(anyhow!(
                "replace() requires at least the source string as the first argument"
            ));
        }
        if pos.len() > 4 {
            return Err(anyhow!(
                "replace() received too many positional arguments (expected at most 4)"
            ));
        }

        let extract_str = |val: &Val, ctx: &str| -> Result<String> {
            match val {
                Val::Str(s) => Ok(s.as_ref().to_string()),
                _ => Err(anyhow!("replace() {} must be a string", ctx)),
            }
        };
        let extract_bool = |val: &Val, ctx: &str| -> Result<bool> {
            match val {
                Val::Bool(b) => Ok(*b),
                _ => Err(anyhow!("replace() {} must be a boolean", ctx)),
            }
        };

        let source = match &pos[0] {
            Val::Str(s) => s.as_ref().to_string(),
            _ => return Err(anyhow!("replace() first argument must be a string")),
        };

        let mut pattern: Option<String> = None;
        let mut with: Option<String> = None;
        let mut all_flag: Option<bool> = None;
        let mut used_named_core = false;

        if pos.len() >= 2 {
            pattern = Some(extract_str(&pos[1], "second argument (pattern)")?);
        }
        if pos.len() >= 3 {
            with = Some(extract_str(&pos[2], "third argument (with)")?);
        }
        if pos.len() >= 4 {
            all_flag = Some(extract_bool(&pos[3], "fourth argument (all flag)")?);
        }

        use std::collections::HashSet;
        let mut seen: HashSet<&str> = HashSet::with_capacity(named.len());
        for (name, value) in named {
            let key = name.as_str();
            if !seen.insert(key) {
                return Err(anyhow!("replace() received duplicate named argument '{}'", name));
            }
            match key {
                "pattern" => {
                    pattern = Some(extract_str(value, "named 'pattern'")?);
                    used_named_core = true;
                }
                "with" => {
                    with = Some(extract_str(value, "named 'with'")?);
                    used_named_core = true;
                }
                "all" => {
                    all_flag = Some(extract_bool(value, "named 'all'")?);
                }
                other => {
                    return Err(anyhow!("replace() does not accept named argument '{}'", other));
                }
            }
        }

        let pattern = pattern.ok_or_else(|| {
            anyhow!("replace() requires a pattern string (provide it positionally or via named 'pattern')")
        })?;
        let with = with.ok_or_else(|| {
            anyhow!("replace() requires a replacement string (provide it positionally or via named 'with')")
        })?;

        let default_all = !used_named_core;
        let all = all_flag.unwrap_or(default_all);

        let result = if all {
            source.replace(pattern.as_str(), with.as_str())
        } else {
            source.replacen(pattern.as_str(), with.as_str(), 1)
        };

        Ok(Val::Str(result.into()))
    }

    fn replace_method(args: &[Val], ctx: &mut VmContext) -> Result<Val> {
        Self::replace(args, &[], ctx)
    }

    /// Extract substring
    fn substring(args: &[Val], _ctx: &mut VmContext) -> Result<Val> {
        if args.len() != 3 {
            return Err(anyhow!("substring() takes exactly 3 arguments: string, start, length"));
        }

        let string = match &args[0] {
            Val::Str(s) => &**s,
            _ => {
                return Err(anyhow!("substring() first argument must be a string"));
            }
        };

        let start = match &args[1] {
            Val::Int(i) => *i as usize,
            _ => {
                return Err(anyhow!("substring() second argument must be an integer"));
            }
        };

        let length = match &args[2] {
            Val::Int(i) => *i as usize,
            _ => {
                return Err(anyhow!("substring() third argument must be an integer"));
            }
        };

        if start > string.len() {
            return Err(anyhow!("substring() start index out of bounds"));
        }

        let end = std::cmp::min(start + length, string.len());
        Ok(Val::Str(string[start..end].into()))
    }

    /// Split string by delimiter
    fn split(args: &[Val], _ctx: &mut VmContext) -> Result<Val> {
        if args.len() != 2 {
            return Err(anyhow!("split() takes exactly 2 arguments: string, delimiter"));
        }

        let string = match &args[0] {
            Val::Str(s) => &**s,
            _ => return Err(anyhow!("split() first argument must be a string")),
        };

        let delimiter = match &args[1] {
            Val::Str(d) => &**d,
            _ => return Err(anyhow!("split() second argument must be a string")),
        };

        let parts: Vec<Val> = if delimiter.is_empty() {
            string.chars().map(|c| Val::Str(c.to_string().into())).collect()
        } else {
            string.split(delimiter).map(|s| Val::Str(s.into())).collect()
        };

        Ok(Val::List(Arc::from(parts)))
    }

    /// Join list of strings with delimiter
    fn join(args: &[Val], _ctx: &mut VmContext) -> Result<Val> {
        if args.len() != 2 {
            return Err(anyhow!("join() takes exactly 2 arguments: list, delimiter"));
        }

        let list = match &args[0] {
            Val::List(l) => &**l,
            _ => return Err(anyhow!("join() first argument must be a list")),
        };

        let delimiter = match &args[1] {
            Val::Str(d) => &**d,
            _ => return Err(anyhow!("join() second argument must be a string")),
        };

        let mut strings = Vec::new();
        for item in list {
            match item {
                Val::Str(s) => strings.push(&**s),
                _ => return Err(anyhow!("join() list must contain only strings")),
            }
        }

        Ok(Val::Str(strings.join(delimiter).into()))
    }
}

impl Module for StringModule {
    fn name(&self) -> &str {
        "string"
    }

    fn description(&self) -> &str {
        "String manipulation functions"
    }

    fn register(&self, _registry: &mut ModuleRegistry) -> Result<()> {
        // Don't register functions globally - they should be accessed via module.function()
        Ok(())
    }

    fn exports(&self) -> HashMap<String, Val> {
        self.functions.clone()
    }
}
