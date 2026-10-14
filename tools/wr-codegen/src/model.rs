//! Name resolution over every parsed assembly: C# type expressions become [`Ty`]s that
//! point at declarations, with generic arguments and inheritance.

use std::collections::HashMap;

use crate::parse::{Kind, RawType, TypeExpr};

/// Built-in value types and `string`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Prim {
    Bool,
    I8,
    U8,
    I16,
    U16,
    I32,
    U32,
    I64,
    U64,
    F32,
    F64,
    Char,
    String,
    Decimal,
    Object,
    IntPtr,
}

impl Prim {
    fn from_name(n: &str) -> Option<Self> {
        Some(match n {
            "bool" | "Boolean" => Self::Bool,
            "sbyte" | "SByte" => Self::I8,
            "byte" | "Byte" => Self::U8,
            "short" | "Int16" => Self::I16,
            "ushort" | "UInt16" => Self::U16,
            "int" | "Int32" => Self::I32,
            "uint" | "UInt32" => Self::U32,
            "long" | "Int64" => Self::I64,
            "ulong" | "UInt64" => Self::U64,
            "float" | "Single" => Self::F32,
            "double" | "Double" => Self::F64,
            "char" | "Char" => Self::Char,
            "string" | "String" => Self::String,
            "decimal" | "Decimal" => Self::Decimal,
            "object" | "Object" => Self::Object,
            "IntPtr" | "nint" | "UIntPtr" | "nuint" => Self::IntPtr,
            _ => return None,
        })
    }

    /// `Type.IsPrimitive`: `string`, `decimal` and `object` are not.
    #[must_use]
    pub fn is_clr_primitive(self) -> bool {
        !matches!(self, Self::String | Self::Decimal | Self::Object)
    }
}

/// A resolved type.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Ty {
    Prim(Prim),
    /// A declaration in the decompiled sources, with generic arguments.
    Decl(usize, Vec<Ty>),
    GenericParam(String),
    /// `T[]`; rank above one is kept as `rank`.
    Array(Box<Ty>, u8),
    /// `System.Collections.Generic.List<T>`.
    List(Box<Ty>),
    /// `System.Nullable<T>` / `T?` on a value type.
    Nullable(Box<Ty>),
    /// A type outside the decompiled sources (mscorlib, System.dll…), by full name.
    External(String, Vec<Ty>),
    Unresolved(String),
}

/// System types we can name without their sources.
const SYSTEM_TYPES: &[(&str, &str)] = &[
    ("List", "System.Collections.Generic"),
    ("Dictionary", "System.Collections.Generic"),
    ("HashSet", "System.Collections.Generic"),
    ("Queue", "System.Collections.Generic"),
    ("Stack", "System.Collections.Generic"),
    ("LinkedList", "System.Collections.Generic"),
    ("SortedList", "System.Collections.Generic"),
    ("SortedDictionary", "System.Collections.Generic"),
    ("IList", "System.Collections.Generic"),
    ("IEnumerable", "System.Collections.Generic"),
    ("ICollection", "System.Collections.Generic"),
    ("IDictionary", "System.Collections.Generic"),
    ("IReadOnlyList", "System.Collections.Generic"),
    ("KeyValuePair", "System.Collections.Generic"),
    ("Nullable", "System"),
    ("Guid", "System"),
    ("DayOfWeek", "System"),
    ("DateTime", "System"),
    ("TimeSpan", "System"),
    ("Type", "System"),
    ("Action", "System"),
    ("Func", "System"),
    ("Lazy", "System"),
    ("Tuple", "System"),
    ("ValueTuple", "System"),
    ("Exception", "System"),
    ("Random", "System"),
    ("WeakReference", "System"),
    ("Array", "System"),
    ("Delegate", "System"),
    ("StringBuilder", "System.Text"),
    ("Regex", "System.Text.RegularExpressions"),
];

#[derive(Debug)]
pub struct Universe {
    pub types: Vec<RawType>,
    /// `Namespace.Outer.Name` + `` `arity `` → candidates (several assemblies may declare it).
    by_full: HashMap<String, Vec<usize>>,
    /// (outer type, `` Name`arity ``) → nested type.
    nested: HashMap<(usize, String), usize>,
    full_names: Vec<String>,
    bases: Vec<Option<Ty>>,
}

fn key(name: &str, arity: usize) -> String {
    if arity == 0 { name.to_owned() } else { format!("{name}`{arity}") }
}

impl Universe {
    #[must_use]
    pub fn new(types: Vec<RawType>) -> Self {
        let mut full_names = Vec::with_capacity(types.len());
        for t in &types {
            let full = match t.outer {
                Some(o) => format!("{}.{}", full_names[o], t.name),
                None if t.namespace.is_empty() => t.name.clone(),
                None => format!("{}.{}", t.namespace, t.name),
            };
            full_names.push(full);
        }
        let mut by_full: HashMap<String, Vec<usize>> = HashMap::new();
        let mut nested = HashMap::new();
        for (i, t) in types.iter().enumerate() {
            by_full.entry(key(&full_names[i], t.generics.len())).or_default().push(i);
            if let Some(o) = t.outer {
                nested.insert((o, key(&t.name, t.generics.len())), i);
            }
        }
        let mut u = Self { types, by_full, nested, full_names, bases: Vec::new() };
        u.bases = (0..u.types.len()).map(|i| u.compute_base(i)).collect();
        u
    }

    #[must_use]
    pub fn full_name(&self, i: usize) -> &str {
        &self.full_names[i]
    }

    /// Picks among same-named declarations, preferring the referencing assembly, then
    /// Assembly-CSharp, then whatever comes first.
    fn pick(&self, cands: &[usize], from: Option<usize>) -> usize {
        if let Some(f) = from {
            let asm = &self.types[f].assembly;
            if let Some(&c) = cands.iter().find(|&&c| &self.types[c].assembly == asm) {
                return c;
            }
        }
        cands
            .iter()
            .copied()
            .find(|&c| &*self.types[c].assembly == "Assembly-CSharp")
            .unwrap_or(cands[0])
    }

    fn lookup_full(&self, full: &str, arity: usize, from: Option<usize>) -> Option<usize> {
        self.by_full.get(&key(full, arity)).map(|c| self.pick(c, from))
    }

    /// Generic parameter names visible inside `ctx` (its own and its enclosing types').
    fn generic_in_scope(&self, ctx: usize, name: &str) -> bool {
        let mut cur = Some(ctx);
        while let Some(c) = cur {
            if self.types[c].generics.iter().any(|g| g == name) {
                return true;
            }
            cur = self.types[c].outer;
        }
        false
    }

    /// Resolves a type expression written inside `ctx` (a type) or at namespace level.
    #[must_use]
    pub fn resolve(&self, e: &TypeExpr, ctx: Option<usize>) -> Ty {
        self.resolve_in(e, ctx, true)
    }

    /// `inh`: look for nested types in base classes too (off while computing bases).
    fn resolve_in(&self, e: &TypeExpr, ctx: Option<usize>, inh: bool) -> Ty {
        match e {
            TypeExpr::Predefined(p) => {
                Prim::from_name(p).map_or_else(|| Ty::Unresolved(p.clone()), Ty::Prim)
            }
            TypeExpr::Array(inner, rank) => {
                Ty::Array(Box::new(self.resolve_in(inner, ctx, inh)), *rank)
            }
            TypeExpr::Nullable(inner) => {
                let t = self.resolve_in(inner, ctx, inh);
                if self.is_value_type(&t) { Ty::Nullable(Box::new(t)) } else { t }
            }
            TypeExpr::Other(s) => Ty::Unresolved(s.clone()),
            TypeExpr::Name { global, segments } => self.resolve_name(*global, segments, ctx, inh),
        }
    }

    fn resolve_args(&self, args: &[TypeExpr], ctx: Option<usize>, inh: bool) -> Vec<Ty> {
        args.iter().map(|a| self.resolve_in(a, ctx, inh)).collect()
    }

    fn resolve_name(
        &self,
        global: bool,
        segs: &[(String, Vec<TypeExpr>)],
        ctx: Option<usize>,
        inh: bool,
    ) -> Ty {
        let (first, first_args) = &segs[0];
        // Single simple name: generic parameter, nested types, namespaces, usings.
        let head = if global { None } else { self.resolve_simple(first, first_args, ctx, inh) };
        if let Some(mut t) = head {
            for (name, args) in &segs[1..] {
                t = match t {
                    Ty::Decl(i, _) => match self.find_nested(i, name, args.len(), inh) {
                        Some(n) => Ty::Decl(n, self.resolve_args(args, ctx, inh)),
                        None => return Ty::Unresolved(self.text(segs)),
                    },
                    _ => return Ty::Unresolved(self.text(segs)),
                };
            }
            return self.normalize(t);
        }
        // Qualified: a namespace prefix, then a type, then nested types. Try absolute and
        // relative to each enclosing namespace.
        let ns_chain: Vec<String> = if global {
            vec![String::new()]
        } else {
            let mut v = Vec::new();
            if let Some(c) = ctx {
                let mut ns = self.types[c].namespace.as_str();
                loop {
                    v.push(ns.to_owned());
                    if ns.is_empty() {
                        break;
                    }
                    ns = ns.rfind('.').map_or("", |p| &ns[..p]);
                }
            } else {
                v.push(String::new());
            }
            v
        };
        for base_ns in &ns_chain {
            for k in (0..segs.len()).rev() {
                let mut ns = base_ns.clone();
                for (s, _) in &segs[..k] {
                    if !ns.is_empty() {
                        ns.push('.');
                    }
                    ns.push_str(s);
                }
                let (tn, targs) = &segs[k];
                let full = if ns.is_empty() { tn.clone() } else { format!("{ns}.{tn}") };
                if let Some(mut i) = self.lookup_full(&full, targs.len(), ctx) {
                    let mut ty_args = self.resolve_args(targs, ctx, inh);
                    let mut ok = true;
                    for (name, args) in &segs[k + 1..] {
                        match self.find_nested(i, name, args.len(), inh) {
                            Some(n) => {
                                i = n;
                                ty_args = self.resolve_args(args, ctx, inh);
                            }
                            None => {
                                ok = false;
                                break;
                            }
                        }
                    }
                    if ok {
                        return self.normalize(Ty::Decl(i, ty_args));
                    }
                }
                if k + 1 == segs.len()
                    && let Some(t) = self.external(&full, targs, ctx, inh)
                {
                    return t;
                }
            }
        }
        Ty::Unresolved(self.text(segs))
    }

    fn text(&self, segs: &[(String, Vec<TypeExpr>)]) -> String {
        TypeExpr::Name { global: false, segments: segs.to_vec() }.text()
    }

    /// Recognizes a few System types by full name.
    fn external(&self, full: &str, args: &[TypeExpr], ctx: Option<usize>, inh: bool) -> Option<Ty> {
        let (ns, name) = full.rsplit_once('.').unwrap_or(("", full));
        if ns == "System"
            && let Some(p) = Prim::from_name(name)
            && args.is_empty()
        {
            return Some(Ty::Prim(p));
        }
        let known = SYSTEM_TYPES.iter().any(|(n, s)| *n == name && *s == ns);
        if !known {
            return None;
        }
        let a = self.resolve_args(args, ctx, inh);
        Some(self.normalize(Ty::External(full.to_owned(), a)))
    }

    fn normalize(&self, t: Ty) -> Ty {
        match t {
            Ty::External(n, mut a) if n == "System.Collections.Generic.List" && a.len() == 1 => {
                Ty::List(Box::new(a.remove(0)))
            }
            Ty::External(n, mut a) if n == "System.Nullable" && a.len() == 1 => {
                Ty::Nullable(Box::new(a.remove(0)))
            }
            t => t,
        }
    }

    fn resolve_simple(
        &self,
        name: &str,
        args: &[TypeExpr],
        ctx: Option<usize>,
        inh: bool,
    ) -> Option<Ty> {
        let arity = args.len();
        if arity == 0
            && let Some(c) = ctx
            && self.generic_in_scope(c, name)
        {
            return Some(Ty::GenericParam(name.to_owned()));
        }
        // Nested types of the context, its enclosing types, and their bases.
        if let Some(c) = ctx {
            let mut cur = Some(c);
            while let Some(t) = cur {
                if let Some(n) = self.find_nested(t, name, arity, inh) {
                    return Some(Ty::Decl(n, self.resolve_args(args, ctx, inh)));
                }
                cur = self.types[t].outer;
            }
        }
        // Namespace chain.
        let ns0 = ctx.map_or("", |c| self.types[c].namespace.as_str());
        let mut ns = ns0;
        loop {
            let full = if ns.is_empty() { name.to_owned() } else { format!("{ns}.{name}") };
            if let Some(i) = self.lookup_full(&full, arity, ctx) {
                return Some(Ty::Decl(i, self.resolve_args(args, ctx, inh)));
            }
            if ns.is_empty() {
                break;
            }
            ns = ns.rfind('.').map_or("", |p| &ns[..p]);
        }
        let usings = ctx.map(|c| self.types[c].usings.clone());
        if let Some(u) = &usings {
            if arity == 0
                && let Some((_, target)) = u.aliases.iter().find(|(a, _)| a == name)
            {
                // Aliases resolve at namespace level, without the context's nesting.
                return Some(self.resolve_in(target, None, inh));
            }
            for n in &u.namespaces {
                if let Some(i) = self.lookup_full(&format!("{n}.{name}"), arity, ctx) {
                    return Some(Ty::Decl(i, self.resolve_args(args, ctx, inh)));
                }
            }
            for n in &u.namespaces {
                if let Some(t) = self.external(&format!("{n}.{name}"), args, ctx, inh) {
                    return Some(t);
                }
            }
        }
        if let Some(p) = Prim::from_name(name).filter(|_| arity == 0 && name != "Object") {
            // `Int32` etc. without a `using System;` (ILSpy always adds it, but be lenient).
            return Some(Ty::Prim(p));
        }
        None
    }

    /// A nested type of `t` or of one of its base classes.
    fn find_nested(&self, t: usize, name: &str, arity: usize, inh: bool) -> Option<usize> {
        let mut seen = 0;
        let mut cur = Some(t);
        while let Some(c) = cur {
            if let Some(&n) = self.nested.get(&(c, key(name, arity))) {
                return Some(n);
            }
            if !inh {
                return None;
            }
            seen += 1;
            if seen > 64 {
                return None;
            }
            cur = match self.base_class(c) {
                Some(Ty::Decl(b, _)) => Some(b),
                _ => None,
            };
        }
        None
    }

    fn compute_base(&self, i: usize) -> Option<Ty> {
        let t = &self.types[i];
        if !matches!(t.kind, Kind::Class | Kind::Record) {
            return None;
        }
        let first = t.bases.first()?;
        // Resolved without inherited nested types, so computing bases can't recurse.
        let r = self.resolve_in(first, Some(i), false);
        match &r {
            Ty::Decl(b, _) if matches!(self.types[*b].kind, Kind::Class | Kind::Record) => Some(r),
            _ => None,
        }
    }

    /// The base class of a declaration (interfaces skipped), unsubstituted.
    #[must_use]
    pub fn base_class(&self, i: usize) -> Option<Ty> {
        self.bases[i].clone()
    }

    /// Whether a resolved type is a value type (struct, enum, primitive except string/object).
    #[must_use]
    pub fn is_value_type(&self, t: &Ty) -> bool {
        match t {
            Ty::Prim(p) => !matches!(p, Prim::String | Prim::Object),
            Ty::Decl(i, _) => matches!(self.types[*i].kind, Kind::Struct | Kind::Enum),
            Ty::Nullable(_) => true,
            Ty::External(n, _) => matches!(
                n.as_str(),
                "System.Guid"
                    | "System.DayOfWeek"
                    | "System.DateTime"
                    | "System.TimeSpan"
                    | "System.Collections.Generic.KeyValuePair"
                    | "System.ValueTuple"
            ),
            _ => false,
        }
    }

    /// Substitutes generic parameters of declaration `decl` with `args`.
    #[must_use]
    pub fn subst(&self, t: &Ty, decl: usize, args: &[Ty]) -> Ty {
        if args.is_empty() {
            return t.clone();
        }
        // Collect parameter names from the declaration and its enclosing types (C# nested
        // generic types carry the outer parameters too, but ILSpy writes them separately;
        // we only substitute the declaration's own).
        let params = &self.types[decl].generics;
        self.subst_map(t, params, args)
    }

    fn subst_map(&self, t: &Ty, params: &[String], args: &[Ty]) -> Ty {
        match t {
            Ty::GenericParam(n) => params
                .iter()
                .position(|p| p == n)
                .and_then(|i| args.get(i).cloned())
                .unwrap_or_else(|| t.clone()),
            Ty::Decl(i, a) => {
                Ty::Decl(*i, a.iter().map(|x| self.subst_map(x, params, args)).collect())
            }
            Ty::Array(e, r) => Ty::Array(Box::new(self.subst_map(e, params, args)), *r),
            Ty::List(e) => Ty::List(Box::new(self.subst_map(e, params, args))),
            Ty::Nullable(e) => Ty::Nullable(Box::new(self.subst_map(e, params, args))),
            Ty::External(n, a) => {
                Ty::External(n.clone(), a.iter().map(|x| self.subst_map(x, params, args)).collect())
            }
            _ => t.clone(),
        }
    }

    /// The base class of a closed type, with the type's generic arguments substituted.
    #[must_use]
    pub fn closed_base(&self, t: &Ty) -> Option<Ty> {
        let Ty::Decl(i, args) = t else { return None };
        let b = self.base_class(*i)?;
        Some(self.subst(&b, *i, args))
    }

    /// `t == target || t.IsSubclassOf(target)` by declaration full name.
    #[must_use]
    pub fn is_or_subclass_of(&self, t: &Ty, target_full: &str) -> bool {
        let mut cur = Some(t.clone());
        let mut n = 0;
        while let Some(Ty::Decl(i, _)) = &cur {
            if self.full_names[*i] == target_full {
                return true;
            }
            n += 1;
            if n > 64 {
                return false;
            }
            cur = self.closed_base(cur.as_ref().unwrap_or(t));
        }
        false
    }

    /// Whether `t`, a base class of it, or an interface any of them lists (transitively)
    /// is the declaration `iface_full`: `typeof(I).IsAssignableFrom(t)` by name, ignoring
    /// generic arguments.
    #[must_use]
    pub fn implements(&self, t: &Ty, iface_full: &str) -> bool {
        let mut stack: Vec<usize> = Vec::new();
        let mut cur = Some(t.clone());
        let mut n = 0;
        while let Some(Ty::Decl(i, _)) = &cur {
            stack.push(*i);
            n += 1;
            if n > 64 {
                break;
            }
            cur = self.closed_base(cur.as_ref().unwrap_or(t));
        }
        let mut seen = std::collections::HashSet::new();
        while let Some(d) = stack.pop() {
            if !seen.insert(d) {
                continue;
            }
            if self.full_names[d] == iface_full {
                return true;
            }
            for b in &self.types[d].bases {
                if let Ty::Decl(j, _) = self.resolve(b, Some(d)) {
                    stack.push(j);
                }
            }
        }
        false
    }

    /// Finds the closed base type whose declaration is `target_full` (e.g. the
    /// `BlueprintReference<T>` above a typed reference class).
    #[must_use]
    pub fn find_base(&self, t: &Ty, target_full: &str) -> Option<Ty> {
        let mut cur = Some(t.clone());
        let mut n = 0;
        while let Some(c) = cur {
            if let Ty::Decl(i, _) = &c
                && self.full_names[*i] == target_full
            {
                return Some(c);
            }
            n += 1;
            if n > 64 {
                return None;
            }
            cur = self.closed_base(&c);
        }
        None
    }

    /// Declarations with this full name (any arity 0).
    #[must_use]
    pub fn find(&self, full: &str) -> Option<usize> {
        self.lookup_full(full, 0, None)
    }

    /// Display name of a closed type: `Ns.Name<Arg, …>`.
    #[must_use]
    pub fn display(&self, t: &Ty) -> String {
        match t {
            Ty::Prim(p) => format!("{p:?}"),
            Ty::Decl(i, a) => {
                let base = &self.full_names[*i];
                if a.is_empty() {
                    base.clone()
                } else {
                    format!(
                        "{base}<{}>",
                        a.iter().map(|x| self.display(x)).collect::<Vec<_>>().join(", ")
                    )
                }
            }
            Ty::GenericParam(n) => n.clone(),
            Ty::Array(e, r) => {
                format!("{}[{}]", self.display(e), ",".repeat(usize::from(r.saturating_sub(1))))
            }
            Ty::List(e) => format!("List<{}>", self.display(e)),
            Ty::Nullable(e) => format!("{}?", self.display(e)),
            Ty::External(n, a) => {
                if a.is_empty() {
                    n.clone()
                } else {
                    format!(
                        "{n}<{}>",
                        a.iter().map(|x| self.display(x)).collect::<Vec<_>>().join(", ")
                    )
                }
            }
            Ty::Unresolved(s) => format!("?{s}"),
        }
    }
}
