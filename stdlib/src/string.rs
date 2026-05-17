use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{Result, anyhow};
use lk_core::{
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
        functions.insert("reverse".to_string(), Val::RustFunction(Self::reverse));
        functions.insert("repeat".to_string(), Val::RustFunction(Self::repeat));
        functions.insert("char".to_string(), Val::RustFunction(Self::char_at));
        functions.insert("byte".to_string(), Val::RustFunction(Self::byte_at));
        functions.insert("chars".to_string(), Val::RustFunction(Self::chars));
        functions.insert("find".to_string(), Val::RustFunction(Self::find));
        functions.insert("is_empty".to_string(), Val::RustFunction(Self::is_empty));
        functions.insert("format".to_string(), Val::RustFunction(Self::format));

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

        register_method("String", "reverse", Self::reverse);
        register_method("String", "repeat", Self::repeat);
        register_method("String", "char", Self::char_at);
        register_method("String", "byte", Self::byte_at);
        register_method("String", "chars", Self::chars);
        register_method("String", "find", Self::find);
        register_method("String", "is_empty", Self::is_empty);

        Self { functions }
    }

    /// Get string length
    fn len(args: &[Val], _ctx: &mut VmContext) -> Result<Val> {
        if args.len() != 1 {
            return Err(anyhow!("len() takes exactly 1 argument"));
        }

        match args[0].as_str() {
            Some(s) => Ok(Val::Int(s.len() as i64)),
            None => Err(anyhow!("len() argument must be a string")),
        }
    }

    /// Convert to lowercase
    fn lower(args: &[Val], _ctx: &mut VmContext) -> Result<Val> {
        if args.len() != 1 {
            return Err(anyhow!("lower() takes exactly 1 argument"));
        }

        match args[0].as_str() {
            Some(s) => Ok(Val::from_str(&s.to_lowercase())),
            None => Err(anyhow!("lower() argument must be a string")),
        }
    }

    /// Convert to uppercase
    fn upper(args: &[Val], _ctx: &mut VmContext) -> Result<Val> {
        if args.len() != 1 {
            return Err(anyhow!("upper() takes exactly 1 argument"));
        }

        match args[0].as_str() {
            Some(s) => Ok(Val::from_str(&s.to_uppercase())),
            None => Err(anyhow!("upper() argument must be a string")),
        }
    }

    /// Trim whitespace from both ends
    fn trim(args: &[Val], _ctx: &mut VmContext) -> Result<Val> {
        if args.len() != 1 {
            return Err(anyhow!("trim() takes exactly 1 argument"));
        }

        match args[0].as_str() {
            Some(s) => Ok(Val::from_str(s.trim())),
            None => Err(anyhow!("trim() argument must be a string")),
        }
    }

    /// Check if string starts with prefix
    fn starts_with(args: &[Val], _ctx: &mut VmContext) -> Result<Val> {
        if args.len() != 2 {
            return Err(anyhow!("starts_with() takes exactly 2 arguments: string, prefix"));
        }

        let string = args[0]
            .as_str()
            .ok_or_else(|| anyhow!("starts_with() first argument must be a string"))?;
        let prefix = args[1]
            .as_str()
            .ok_or_else(|| anyhow!("starts_with() second argument must be a string"))?;
        Ok(Val::Bool(string.starts_with(prefix)))
    }

    /// Check if string ends with suffix
    fn ends_with(args: &[Val], _ctx: &mut VmContext) -> Result<Val> {
        if args.len() != 2 {
            return Err(anyhow!("ends_with() takes exactly 2 arguments: string, suffix"));
        }

        let string = args[0]
            .as_str()
            .ok_or_else(|| anyhow!("ends_with() first argument must be a string"))?;
        let suffix = args[1]
            .as_str()
            .ok_or_else(|| anyhow!("ends_with() second argument must be a string"))?;
        Ok(Val::Bool(string.ends_with(suffix)))
    }

    /// Check if string contains substring
    fn contains(args: &[Val], _ctx: &mut VmContext) -> Result<Val> {
        if args.len() != 2 {
            return Err(anyhow!("contains() takes exactly 2 arguments: string, substring"));
        }

        let string = args[0]
            .as_str()
            .ok_or_else(|| anyhow!("contains() first argument must be a string"))?;
        let substring = args[1]
            .as_str()
            .ok_or_else(|| anyhow!("contains() second argument must be a string"))?;
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
            val.as_str()
                .ok_or_else(|| anyhow!("replace() {} must be a string", ctx))
                .map(str::to_owned)
        };
        let extract_bool = |val: &Val, ctx: &str| -> Result<bool> {
            match val {
                Val::Bool(b) => Ok(*b),
                _ => Err(anyhow!("replace() {} must be a boolean", ctx)),
            }
        };

        let source = pos[0]
            .as_str()
            .ok_or_else(|| anyhow!("replace() first argument must be a string"))?
            .to_owned();

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

        Ok(Val::from_str(&result))
    }

    fn replace_method(args: &[Val], ctx: &mut VmContext) -> Result<Val> {
        Self::replace(args, &[], ctx)
    }

    /// Extract substring
    fn substring(args: &[Val], _ctx: &mut VmContext) -> Result<Val> {
        if args.len() != 3 {
            return Err(anyhow!("substring() takes exactly 3 arguments: string, start, length"));
        }

        let string = args[0]
            .as_str()
            .ok_or_else(|| anyhow!("substring() first argument must be a string"))?;

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
        Ok(Val::from_str(&string[start..end]))
    }

    /// Split string by delimiter
    fn split(args: &[Val], _ctx: &mut VmContext) -> Result<Val> {
        if args.len() != 2 {
            return Err(anyhow!("split() takes exactly 2 arguments: string, delimiter"));
        }

        let string = args[0]
            .as_str()
            .ok_or_else(|| anyhow!("split() first argument must be a string"))?;
        let delimiter = args[1]
            .as_str()
            .ok_or_else(|| anyhow!("split() second argument must be a string"))?;

        let parts: Vec<Val> = if delimiter.is_empty() {
            string.chars().map(|c| Val::from_str(&c.to_string())).collect()
        } else {
            string.split(delimiter).map(Val::from_str).collect()
        };

        Ok(Val::List(Arc::from(parts)))
    }

    /// Reverse a string
    fn reverse(args: &[Val], _ctx: &mut VmContext) -> Result<Val> {
        if args.len() != 1 {
            return Err(anyhow!("reverse() takes exactly 1 argument"));
        }
        match args[0].as_str() {
            Some(s) => Ok(Val::from_str(&s.chars().rev().collect::<String>())),
            None => Err(anyhow!("reverse() argument must be a string")),
        }
    }

    /// Repeat a string n times
    fn repeat(args: &[Val], _ctx: &mut VmContext) -> Result<Val> {
        if args.len() != 2 {
            return Err(anyhow!("repeat() takes exactly 2 arguments: string, count"));
        }
        let s = args[0]
            .as_str()
            .ok_or_else(|| anyhow!("repeat() first argument must be a string"))?;
        let n = match &args[1] {
            Val::Int(i) => *i,
            _ => return Err(anyhow!("repeat() second argument must be an integer")),
        };
        if n < 0 {
            return Err(anyhow!("repeat() count must be non-negative"));
        }
        Ok(Val::from_str(&s.repeat(n as usize)))
    }

    /// Get character at index (returns single-char string)
    fn char_at(args: &[Val], _ctx: &mut VmContext) -> Result<Val> {
        if args.len() != 2 {
            return Err(anyhow!("char() takes exactly 2 arguments: string, index"));
        }
        let s = args[0]
            .as_str()
            .ok_or_else(|| anyhow!("char() first argument must be a string"))?;
        let idx = match &args[1] {
            Val::Int(i) => *i as usize,
            _ => return Err(anyhow!("char() second argument must be an integer")),
        };
        match s.chars().nth(idx) {
            Some(c) => Ok(Val::from_str(&c.to_string())),
            None => Ok(Val::Nil),
        }
    }

    /// Get byte value of character at index
    fn byte_at(args: &[Val], _ctx: &mut VmContext) -> Result<Val> {
        if args.len() != 2 {
            return Err(anyhow!("byte() takes exactly 2 arguments: string, index"));
        }
        let s = args[0]
            .as_str()
            .ok_or_else(|| anyhow!("byte() first argument must be a string"))?;
        let idx = match &args[1] {
            Val::Int(i) => *i as usize,
            _ => return Err(anyhow!("byte() second argument must be an integer")),
        };
        match s.as_bytes().get(idx) {
            Some(b) => Ok(Val::Int(*b as i64)),
            None => Ok(Val::Nil),
        }
    }

    /// Convert string to list of characters
    fn chars(args: &[Val], _ctx: &mut VmContext) -> Result<Val> {
        if args.len() != 1 {
            return Err(anyhow!("chars() takes exactly 1 argument"));
        }
        match args[0].as_str() {
            Some(s) => {
                let list: Vec<Val> = s.chars().map(|c| Val::from_str(&c.to_string())).collect();
                Ok(Val::List(Arc::from(list)))
            }
            None => Err(anyhow!("chars() argument must be a string")),
        }
    }

    /// Find substring position (returns index or nil)
    fn find(args: &[Val], _ctx: &mut VmContext) -> Result<Val> {
        if args.len() != 2 && args.len() != 3 {
            return Err(anyhow!("find() takes 2 or 3 arguments: string, pattern[, start]"));
        }
        let s = args[0]
            .as_str()
            .ok_or_else(|| anyhow!("find() first argument must be a string"))?;
        let pattern = args[1]
            .as_str()
            .ok_or_else(|| anyhow!("find() second argument must be a string"))?;
        let start = if args.len() >= 3 {
            match &args[2] {
                Val::Int(i) => *i as usize,
                _ => return Err(anyhow!("find() third argument must be an integer")),
            }
        } else {
            0
        };
        if start > s.len() {
            return Ok(Val::Nil);
        }
        match s[start..].find(pattern) {
            Some(idx) => Ok(Val::Int((start + idx) as i64)),
            None => Ok(Val::Nil),
        }
    }

    /// Check if string is empty
    fn is_empty(args: &[Val], _ctx: &mut VmContext) -> Result<Val> {
        if args.len() != 1 {
            return Err(anyhow!("is_empty() takes exactly 1 argument"));
        }
        match args[0].as_str() {
            Some(s) => Ok(Val::Bool(s.is_empty())),
            None => Err(anyhow!("is_empty() argument must be a string")),
        }
    }

    /// Format string (simple positional formatting)
    fn format(args: &[Val], ctx: &mut VmContext) -> Result<Val> {
        if args.is_empty() {
            return Err(anyhow!("format() requires at least 1 argument (format string)"));
        }
        let fmt = args[0]
            .as_str()
            .ok_or_else(|| anyhow!("format() first argument must be a string"))?
            .to_owned();
        let rest = &args[1..];
        let mut out = String::with_capacity(fmt.len());
        let chars: Vec<char> = fmt.chars().collect();
        let mut i = 0usize;
        let mut arg_idx = 0usize;
        while i < chars.len() {
            if chars[i] == '{' && i + 1 < chars.len() && chars[i + 1] == '}' {
                if arg_idx < rest.len() {
                    out.push_str(&rest[arg_idx].display_string(Some(ctx)));
                    arg_idx += 1;
                } else {
                    out.push_str("{}");
                }
                i += 2;
            } else {
                out.push(chars[i]);
                i += 1;
            }
        }
        // Append any remaining args
        if arg_idx < rest.len() {
            if !out.is_empty() {
                out.push(' ');
            }
            for (j, v) in rest[arg_idx..].iter().enumerate() {
                if j > 0 {
                    out.push(' ');
                }
                out.push_str(&v.display_string(Some(ctx)));
            }
        }
        Ok(Val::from_str(&out))
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

        let delimiter = args[1]
            .as_str()
            .ok_or_else(|| anyhow!("join() second argument must be a string"))?;

        let mut strings: Vec<&str> = Vec::with_capacity(list.len());
        for item in list.iter() {
            match item.as_str() {
                Some(s) => strings.push(s),
                None => return Err(anyhow!("join() list must contain only strings")),
            }
        }

        Ok(Val::from_str(&strings.join(delimiter)))
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
