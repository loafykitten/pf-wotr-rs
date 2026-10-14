//! CLR type names as Newtonsoft writes them in `$type`: `Namespace.Outer+Nested`1[[Arg,
//! Assembly, Version=…],[…]], Assembly, Version=…`. The schema keys types by the name
//! with every assembly qualification removed; the top-level assembly is checked
//! separately (`DefaultSerializationBinder` loads the type from that assembly).

/// A parsed `$type`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeName {
    /// The name without assembly qualifications (the schema key).
    pub key: String,
    /// The simple name of the top-level assembly, if given.
    pub assembly: Option<String>,
    /// Every generic argument at any depth: its key and assembly simple name, if given.
    pub args: Vec<(String, Option<String>)>,
}

/// Deepest generic nesting accepted (deeper names are refused, not recursed into).
pub const MAX_NESTING: usize = 32;

/// Parses a `$type` value. `None` if the brackets don't balance.
#[must_use]
pub fn parse(s: &str) -> Option<TypeName> {
    let b = s.as_bytes();
    let mut key = String::with_capacity(s.len());
    let mut args = Vec::new();
    let end = name(b, 0, &mut key, &mut args, 0)?;
    let assembly = if end < b.len() {
        if b[end] != b',' {
            return None;
        }
        let rest = s[end + 1..].trim();
        Some(rest.split(',').next().unwrap_or("").trim().to_owned())
    } else {
        None
    };
    Some(TypeName { key, assembly, args })
}

/// Reads a type name starting at `i` into `out`; returns the index after it (at a `,`,
/// `]` or the end).
fn name(
    b: &[u8],
    mut i: usize,
    out: &mut String,
    args: &mut Vec<(String, Option<String>)>,
    depth: usize,
) -> Option<usize> {
    if depth > MAX_NESTING {
        return None;
    }
    let start = i;
    while i < b.len() && !matches!(b[i], b'[' | b',' | b']') {
        i += 1;
    }
    out.push_str(std::str::from_utf8(&b[start..i]).ok()?.trim());
    while i < b.len() && b[i] == b'[' {
        if b.get(i + 1) == Some(&b'[') {
            // Generic arguments: `[[A, asm],[B, asm]]`.
            out.push('[');
            i += 1;
            loop {
                if b.get(i) != Some(&b'[') {
                    return None;
                }
                out.push('[');
                let mut arg = String::new();
                i = name(b, i + 1, &mut arg, args, depth + 1)?;
                out.push_str(&arg);
                // An assembly qualification up to this argument's `]`.
                let mut asm = None;
                if b.get(i) == Some(&b',') {
                    let s0 = i + 1;
                    let mut d = 0usize;
                    while i < b.len() {
                        match b[i] {
                            b'[' => d += 1,
                            b']' if d == 0 => break,
                            b']' => d -= 1,
                            _ => {}
                        }
                        i += 1;
                    }
                    let q = std::str::from_utf8(&b[s0..i.min(b.len())]).ok()?;
                    asm = Some(q.split(',').next().unwrap_or("").trim().to_owned());
                }
                args.push((arg, asm));
                if b.get(i) != Some(&b']') {
                    return None;
                }
                out.push(']');
                i += 1;
                match b.get(i) {
                    Some(b',') => {
                        out.push(',');
                        i += 1;
                    }
                    Some(b']') => {
                        out.push(']');
                        i += 1;
                        break;
                    }
                    _ => return None,
                }
            }
        } else {
            // Array suffix: `[]`, `[,]`.
            let s = i;
            while i < b.len() && b[i] != b']' {
                i += 1;
            }
            if i >= b.len() {
                return None;
            }
            i += 1;
            out.push_str(std::str::from_utf8(&b[s..i]).ok()?);
        }
    }
    Some(i)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_and_generic_names() {
        let t = parse("Kingmaker.UnitLogic.Feature, Assembly-CSharp").unwrap();
        assert_eq!(t.key, "Kingmaker.UnitLogic.Feature");
        assert_eq!(t.assembly.as_deref(), Some("Assembly-CSharp"));
        let g = "Kingmaker.EntitySystem.EntityFactComponentDelegate`2+ComponentRuntime[[Kingmaker.EntitySystem.Entities.UnitEntityData, Assembly-CSharp, Version=0.0.0.0, Culture=neutral, PublicKeyToken=null],[Kingmaker.UnitLogic.FactLogic.AddFactsData, Assembly-CSharp, Version=0.0.0.0, Culture=neutral, PublicKeyToken=null]], Assembly-CSharp, Version=0.0.0.0, Culture=neutral, PublicKeyToken=null";
        let t = parse(g).unwrap();
        assert_eq!(
            t.key,
            "Kingmaker.EntitySystem.EntityFactComponentDelegate`2+ComponentRuntime[[Kingmaker.EntitySystem.Entities.UnitEntityData],[Kingmaker.UnitLogic.FactLogic.AddFactsData]]"
        );
        assert_eq!(t.assembly.as_deref(), Some("Assembly-CSharp"));
        assert_eq!(t.args.len(), 2);
        assert_eq!(t.args[1].1.as_deref(), Some("Assembly-CSharp"));
        let n = parse("System.Nullable`1[[UnityEngine.Vector3, UnityEngine.CoreModule]], mscorlib")
            .unwrap();
        assert_eq!(n.key, "System.Nullable`1[[UnityEngine.Vector3]]");
        assert_eq!(n.assembly.as_deref(), Some("mscorlib"));
        let nested = parse("A`1[[B`1[[C, x]], y]], z").unwrap();
        assert_eq!(nested.key, "A`1[[B`1[[C]]]]");
        assert_eq!(parse("X[], asm").unwrap().key, "X[]");
        assert_eq!(parse("NoAssembly").unwrap().assembly, None);
        assert!(parse("Broken[[A").is_none());
        let deep = format!("{}X{}", "A`1[[".repeat(100), "]]".repeat(100));
        assert!(parse(&deep).is_none(), "nesting is bounded");
    }
}
