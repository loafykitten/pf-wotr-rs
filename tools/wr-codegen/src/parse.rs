//! C# declarations from tree-sitter: types, fields, attributes, enum members and the
//! lifecycle methods a loader cares about. Bodies are skipped; only declarations matter.

use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result, anyhow};
use tree_sitter::{Node, Parser};

/// A type as written in the source, before name resolution.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TypeExpr {
    /// `int`, `string`, `float`…
    Predefined(String),
    /// `A.B<C>.D`: each segment with its type arguments. `global` for `global::`.
    Name {
        global: bool,
        segments: Vec<(String, Vec<TypeExpr>)>,
    },
    Array(Box<TypeExpr>, u8),
    Nullable(Box<TypeExpr>),
    /// Tuples, pointers, function pointers: never serialized.
    Other(String),
}

impl TypeExpr {
    #[must_use]
    pub fn text(&self) -> String {
        match self {
            Self::Predefined(s) | Self::Other(s) => s.clone(),
            Self::Name { global, segments } => {
                let mut out = if *global { "global::".to_owned() } else { String::new() };
                for (i, (name, args)) in segments.iter().enumerate() {
                    if i > 0 {
                        out.push('.');
                    }
                    out.push_str(name);
                    if !args.is_empty() {
                        out.push('<');
                        out.push_str(&args.iter().map(Self::text).collect::<Vec<_>>().join(", "));
                        out.push('>');
                    }
                }
                out
            }
            Self::Array(e, rank) => {
                format!("{}[{}]", e.text(), ",".repeat(usize::from(rank.saturating_sub(1))))
            }
            Self::Nullable(e) => format!("{}?", e.text()),
        }
    }
}

/// A constant expression, kept as a tree so enum members can refer to each other.
#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    /// An integer literal and its suffix: (`u`/`U` present, `l`/`L` present).
    Int(i128, (bool, bool)),
    Float(f64),
    Bool(bool),
    Str(String),
    Char(u32),
    Null,
    /// A name or member access such as `Flags.A` or `int.MaxValue`.
    Ident(Vec<String>),
    Unary(String, Box<Expr>),
    Binary(String, Box<Expr>, Box<Expr>),
    Cast(TypeExpr, Box<Expr>),
    /// `new T()` with no arguments and no initializer.
    NewEmpty(TypeExpr),
    /// `new T(a, b, …)` with constant arguments and no initializer.
    New(TypeExpr, Vec<Expr>),
    /// `default` or `default(T)`.
    Default,
    /// Anything else (calls, `new T(args)`, collection initializers…), as source text.
    Other(String),
}

#[derive(Debug, Clone)]
pub struct Attr {
    /// Last segment, without an `Attribute` suffix: `JsonProperty`, `SerializeField`.
    pub name: String,
    /// Positional (`None`) and named arguments.
    pub args: Vec<(Option<String>, Expr)>,
}

impl Attr {
    #[must_use]
    pub fn string_arg(&self, named: &str) -> Option<&str> {
        self.args.iter().find_map(|(n, e)| match (n.as_deref(), e) {
            (Some(n), Expr::Str(s)) if n == named => Some(s.as_str()),
            _ => None,
        })
    }

    #[must_use]
    pub fn first_positional_string(&self) -> Option<&str> {
        self.args.iter().find_map(|(n, e)| match (n, e) {
            (None, Expr::Str(s)) => Some(s.as_str()),
            _ => None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    Class,
    Struct,
    Interface,
    Enum,
    Record,
    Delegate,
}

#[derive(Debug, Clone)]
pub struct RawField {
    pub name: String,
    pub ty: TypeExpr,
    pub modifiers: Vec<String>,
    pub attrs: Vec<Attr>,
    pub init: Option<Expr>,
}

impl RawField {
    #[must_use]
    pub fn has_mod(&self, m: &str) -> bool {
        self.modifiers.iter().any(|x| x == m)
    }
}

/// A property declaration (Newtonsoft serializes properties with `[JsonProperty]`).
#[derive(Debug, Clone)]
pub struct RawProp {
    pub name: String,
    pub ty: TypeExpr,
    pub modifiers: Vec<String>,
    pub attrs: Vec<Attr>,
    /// Has a `get` accessor (or is expression-bodied).
    pub getter: bool,
    /// Has a `set` or `init` accessor, of any visibility.
    pub setter: bool,
    /// `I.Name` explicit interface implementation.
    pub explicit_interface: bool,
}

impl RawProp {
    #[must_use]
    pub fn has_mod(&self, m: &str) -> bool {
        self.modifiers.iter().any(|x| x == m)
    }
}

/// An instance or static constructor's signature.
#[derive(Debug, Clone)]
pub struct RawCtor {
    pub params: Vec<TypeExpr>,
    pub param_names: Vec<String>,
    pub attrs: Vec<String>,
    pub is_static: bool,
    pub is_public: bool,
}

#[derive(Debug, Clone)]
pub struct RawMethod {
    pub name: String,
    pub attrs: Vec<String>,
    pub params: usize,
    /// Body is present and not empty (`{ }` and `;` count as empty).
    pub has_body: bool,
}

/// `using` directives in scope for a file.
#[derive(Debug, Default)]
pub struct Usings {
    pub namespaces: Vec<String>,
    pub aliases: Vec<(String, TypeExpr)>,
    pub statics: Vec<TypeExpr>,
}

#[derive(Debug, Clone)]
pub struct RawType {
    pub assembly: Arc<str>,
    pub file: Arc<str>,
    pub line: u32,
    pub namespace: String,
    /// Index (within the same file's list) of the enclosing type.
    pub outer: Option<usize>,
    pub name: String,
    pub kind: Kind,
    pub generics: Vec<String>,
    pub bases: Vec<TypeExpr>,
    pub attrs: Vec<Attr>,
    pub modifiers: Vec<String>,
    pub fields: Vec<RawField>,
    pub props: Vec<RawProp>,
    pub ctors: Vec<RawCtor>,
    pub enum_members: Vec<(String, Option<Expr>)>,
    pub methods: Vec<RawMethod>,
    /// A parameterless constructor with statements in its body.
    pub ctor_with_body: bool,
    /// `Field = expr;` / `this.Field = expr;` statements in the parameterless constructor.
    pub ctor_assigns: Vec<(String, Expr)>,
    /// The parameterless constructor has statements other than such assignments.
    pub ctor_other: bool,
    pub usings: Arc<Usings>,
}

impl RawType {
    #[must_use]
    pub fn has_attr(&self, name: &str) -> bool {
        self.attrs.iter().any(|a| a.name == name)
    }

    #[must_use]
    pub fn has_mod(&self, m: &str) -> bool {
        self.modifiers.iter().any(|x| x == m)
    }
}

/// Files whose syntax tree has an error or missing node (declarations inside the broken
/// region may be lost). Read after parsing.
pub static PARSE_ERRORS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
/// `file:line` of the first error in each such file.
pub static ERROR_FILES: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

/// Parses one file. Types are returned in declaration order; `outer` indexes into the
/// returned list.
pub fn parse_file(assembly: &Arc<str>, rel: &str, src: &str) -> Result<Vec<RawType>> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_c_sharp::LANGUAGE.into())
        .map_err(|e| anyhow!("load grammar: {e}"))?;
    let tree = parser.parse(src, None).ok_or_else(|| anyhow!("parse failed"))?;
    let mut cx = Cx {
        src,
        assembly: assembly.clone(),
        file: Arc::from(rel),
        out: Vec::new(),
        usings: Arc::new(Usings::default()),
    };
    let root = tree.root_node();
    if root.has_error() {
        PARSE_ERRORS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let mut at = None;
        let mut stack = vec![root];
        while let Some(n) = stack.pop() {
            if n.is_error() || n.is_missing() {
                at = Some(n.start_position().row + 1);
                break;
            }
            let mut c = n.walk();
            stack.extend(n.children(&mut c).filter(|x| x.has_error() || x.is_missing()));
        }
        if let Ok(mut v) = ERROR_FILES.lock() {
            v.push(format!("{rel}:{}", at.unwrap_or(0)));
        }
    }
    cx.usings = Arc::new(collect_usings(&cx, root));
    cx.members(root, "", None);
    Ok(cx.out)
}

/// Parses every `.cs` under `dir` (one assembly). Files are read in parallel.
pub fn parse_assembly(dir: &Path) -> Result<Vec<RawType>> {
    use rayon::prelude::*;
    let assembly: Arc<str> =
        Arc::from(dir.file_name().and_then(|n| n.to_str()).ok_or_else(|| anyhow!("bad dir"))?);
    let mut files = Vec::new();
    collect_cs(dir, &mut files)?;
    files.sort();
    let parts: Vec<Result<Vec<RawType>>> = files
        .par_iter()
        .map(|f| {
            let src = std::fs::read_to_string(f).with_context(|| f.display().to_string())?;
            let rel = f.strip_prefix(dir.parent().unwrap_or(dir)).unwrap_or(f);
            parse_file(&assembly, &rel.to_string_lossy(), &src)
                .with_context(|| f.display().to_string())
        })
        .collect();
    let mut out = Vec::new();
    for p in parts {
        let mut types = p?;
        // Rebase `outer` indices into the assembly-wide list.
        let base = out.len();
        for t in &mut types {
            if let Some(o) = t.outer.as_mut() {
                *o += base;
            }
        }
        out.extend(types);
    }
    Ok(out)
}

fn collect_cs(dir: &Path, out: &mut Vec<std::path::PathBuf>) -> Result<()> {
    for e in std::fs::read_dir(dir)? {
        let p = e?.path();
        if p.is_dir() {
            collect_cs(&p, out)?;
        } else if p.extension().is_some_and(|x| x == "cs") {
            out.push(p);
        }
    }
    Ok(())
}

struct Cx<'s> {
    src: &'s str,
    assembly: Arc<str>,
    file: Arc<str>,
    out: Vec<RawType>,
    usings: Arc<Usings>,
}

fn collect_usings(cx: &Cx<'_>, root: Node<'_>) -> Usings {
    let mut u = Usings::default();
    let mut stack = vec![root];
    while let Some(n) = stack.pop() {
        let mut c = n.walk();
        for ch in n.named_children(&mut c) {
            match ch.kind() {
                "using_directive" => {
                    let is_static = (0..ch.child_count())
                        .filter_map(|i| ch.child(i))
                        .any(|x| x.kind() == "static");
                    if let Some(alias) = ch.child_by_field_name("name") {
                        let target = (0..ch.named_child_count())
                            .filter_map(|i| ch.named_child(i as u32))
                            .find(|x| x.id() != alias.id());
                        if let Some(t) = target {
                            u.aliases.push((cx.text(alias).to_owned(), cx.type_expr(t)));
                        }
                    } else if let Some(t) = ch.named_child(0) {
                        if is_static {
                            u.statics.push(cx.type_expr(t));
                        } else {
                            u.namespaces.push(cx.text(t).replace(char::is_whitespace, ""));
                        }
                    }
                }
                // Usings inside a block namespace apply to it; ILSpy puts them at the top,
                // so merging them into the file's list is close enough.
                "namespace_declaration" | "declaration_list" => stack.push(ch),
                _ => {}
            }
        }
    }
    u
}

impl<'s> Cx<'s> {
    fn text(&self, n: Node<'_>) -> &'s str {
        &self.src[n.byte_range()]
    }

    fn line(n: Node<'_>) -> u32 {
        u32::try_from(n.start_position().row + 1).unwrap_or(u32::MAX)
    }

    /// Walks a compilation unit, namespace body or type body for declarations.
    fn members(&mut self, n: Node<'_>, ns: &str, outer: Option<usize>) {
        let mut ns = ns.to_owned();
        let mut c = n.walk();
        let children: Vec<Node<'_>> = n.named_children(&mut c).collect();
        for ch in children {
            match ch.kind() {
                "file_scoped_namespace_declaration" => {
                    if let Some(name) = ch.child_by_field_name("name") {
                        ns = join_ns(&ns, &self.text(name).replace(char::is_whitespace, ""));
                    }
                    // In some grammar versions the members are children of this node.
                    self.members(ch, &ns, outer);
                }
                "namespace_declaration" => {
                    let inner = ch.child_by_field_name("name").map_or_else(
                        || ns.clone(),
                        |name| join_ns(&ns, &self.text(name).replace(char::is_whitespace, "")),
                    );
                    if let Some(body) = ch.child_by_field_name("body") {
                        self.members(body, &inner, outer);
                    }
                }
                "class_declaration"
                | "struct_declaration"
                | "interface_declaration"
                | "enum_declaration"
                | "record_declaration"
                | "record_struct_declaration"
                | "delegate_declaration" => self.type_decl(ch, &ns, outer),
                "declaration_list" => self.members(ch, &ns, outer),
                _ => {}
            }
        }
    }

    fn type_decl(&mut self, n: Node<'_>, ns: &str, outer: Option<usize>) {
        let kind = match n.kind() {
            "class_declaration" => Kind::Class,
            "struct_declaration" | "record_struct_declaration" => Kind::Struct,
            "interface_declaration" => Kind::Interface,
            "enum_declaration" => Kind::Enum,
            "record_declaration" => Kind::Record,
            _ => Kind::Delegate,
        };
        let name = n.child_by_field_name("name").map_or("", |x| self.text(x)).to_owned();
        let mut t = RawType {
            assembly: self.assembly.clone(),
            file: self.file.clone(),
            line: Self::line(n),
            namespace: ns.to_owned(),
            outer,
            name,
            kind,
            generics: Vec::new(),
            bases: Vec::new(),
            attrs: Vec::new(),
            modifiers: Vec::new(),
            fields: Vec::new(),
            props: Vec::new(),
            ctors: Vec::new(),
            enum_members: Vec::new(),
            methods: Vec::new(),
            ctor_with_body: false,
            ctor_assigns: Vec::new(),
            ctor_other: false,
            usings: self.usings.clone(),
        };
        let mut body = None;
        let mut c = n.walk();
        for (i, ch) in n.children(&mut c).enumerate() {
            let field = n.field_name_for_child(u32::try_from(i).unwrap_or(u32::MAX));
            match ch.kind() {
                "attribute_list" => t.attrs.extend(self.attrs(ch)),
                "modifier" => t.modifiers.push(self.text(ch).to_owned()),
                "type_parameter_list" => {
                    let mut c2 = ch.walk();
                    for tp in ch.named_children(&mut c2) {
                        if tp.kind() == "type_parameter" {
                            let nm = tp.child_by_field_name("name").unwrap_or(tp);
                            t.generics.push(self.text(nm).to_owned());
                        }
                    }
                }
                "base_list" => {
                    let mut c2 = ch.walk();
                    for b in ch.named_children(&mut c2) {
                        // `primary_constructor_base_type` wraps a type in records.
                        let b = if b.kind() == "primary_constructor_base_type" {
                            b.named_child(0).unwrap_or(b)
                        } else {
                            b
                        };
                        if b.kind() != "argument_list" {
                            t.bases.push(self.type_expr(b));
                        }
                    }
                }
                _ if field == Some("body") => body = Some(ch),
                "declaration_list" | "enum_member_declaration_list" => body = Some(ch),
                _ => {}
            }
        }
        let idx = self.out.len();
        self.out.push(t);
        let Some(body) = body else { return };
        let mut c = body.walk();
        let members: Vec<Node<'_>> = body.named_children(&mut c).collect();
        for m in members {
            match m.kind() {
                "field_declaration" => {
                    let fields = self.field_decl(m);
                    self.out[idx].fields.extend(fields);
                }
                "property_declaration" => {
                    if let Some(p) = self.prop_decl(m) {
                        self.out[idx].props.push(p);
                    }
                }
                "enum_member_declaration" => {
                    let nm = m.child_by_field_name("name").map_or("", |x| self.text(x)).to_owned();
                    let v = m.child_by_field_name("value").map(|v| self.expr(v));
                    self.out[idx].enum_members.push((nm, v));
                }
                "method_declaration" => {
                    let mut attrs = Vec::new();
                    let mut c2 = m.walk();
                    for ch in m.children(&mut c2) {
                        if ch.kind() == "attribute_list" {
                            attrs.extend(self.attrs(ch).into_iter().map(|a| a.name));
                        }
                    }
                    let params = m.child_by_field_name("parameters").map_or(0, |p| {
                        let mut c3 = p.walk();
                        p.named_children(&mut c3).filter(|x| x.kind() == "parameter").count()
                    });
                    let has_body = m.child_by_field_name("body").is_some_and(|b| {
                        b.kind() == "arrow_expression_clause" || b.named_child_count() > 0
                    });
                    let name =
                        m.child_by_field_name("name").map_or("", |x| self.text(x)).to_owned();
                    self.out[idx].methods.push(RawMethod { name, attrs, params, has_body });
                }
                "constructor_declaration" => {
                    let is_static = {
                        let mut c2 = m.walk();
                        m.children(&mut c2)
                            .any(|ch| ch.kind() == "modifier" && self.text(ch) == "static")
                    };
                    let params = m.child_by_field_name("parameters").map_or(0, |p| {
                        let mut c3 = p.walk();
                        p.named_children(&mut c3).filter(|x| x.kind() == "parameter").count()
                    });
                    let has_body = m.child_by_field_name("body").is_some_and(|b| {
                        b.kind() == "arrow_expression_clause" || b.named_child_count() > 0
                    });
                    let ctor = self.ctor_sig(m, is_static);
                    self.out[idx].ctors.push(ctor);
                    if !is_static && params == 0 && has_body {
                        self.out[idx].ctor_with_body = true;
                        let (assigns, other) = m
                            .child_by_field_name("body")
                            .map_or((Vec::new(), false), |b| self.ctor_body(b));
                        self.out[idx].ctor_assigns = assigns;
                        self.out[idx].ctor_other = other;
                    }
                }
                "class_declaration"
                | "struct_declaration"
                | "interface_declaration"
                | "enum_declaration"
                | "record_declaration"
                | "record_struct_declaration"
                | "delegate_declaration" => self.type_decl(m, ns, Some(idx)),
                _ => {}
            }
        }
    }

    /// Simple field assignments in a constructor body, and whether anything else is there.
    fn ctor_body(&self, body: Node<'_>) -> (Vec<(String, Expr)>, bool) {
        let mut out = Vec::new();
        let mut other = false;
        let mut c = body.walk();
        for st in body.named_children(&mut c) {
            let assign = (st.kind() == "expression_statement")
                .then(|| st.named_child(0))
                .flatten()
                .filter(|e| e.kind() == "assignment_expression");
            let Some(a) = assign else {
                other = true;
                continue;
            };
            let op = a.child_by_field_name("operator").map_or("=", |o| self.text(o));
            let left = a.child_by_field_name("left");
            let name = left.and_then(|l| match l.kind() {
                "identifier" => Some(self.text(l).to_owned()),
                "member_access_expression"
                    if l.child_by_field_name("expression")
                        .is_some_and(|e| self.text(e) == "this") =>
                {
                    l.child_by_field_name("name").map(|n| self.text(n).to_owned())
                }
                _ => None,
            });
            match (name, a.child_by_field_name("right"), op) {
                (Some(n), Some(r), "=") => out.push((n, self.expr(r))),
                _ => other = true,
            }
        }
        (out, other)
    }

    fn ctor_sig(&self, m: Node<'_>, is_static: bool) -> RawCtor {
        let mut attrs = Vec::new();
        let mut is_public = false;
        let mut c = m.walk();
        for ch in m.children(&mut c) {
            if ch.kind() == "attribute_list" {
                attrs.extend(self.attrs(ch).into_iter().map(|a| a.name));
            }
            if ch.kind() == "modifier" && self.text(ch) == "public" {
                is_public = true;
            }
        }
        let mut params = Vec::new();
        let mut param_names = Vec::new();
        if let Some(p) = m.child_by_field_name("parameters") {
            let mut c3 = p.walk();
            for x in p.named_children(&mut c3).filter(|x| x.kind() == "parameter") {
                params.push(
                    x.child_by_field_name("type")
                        .map_or_else(|| TypeExpr::Other(String::new()), |t| self.type_expr(t)),
                );
                param_names
                    .push(x.child_by_field_name("name").map_or("", |n| self.text(n)).to_owned());
            }
        }
        RawCtor { params, param_names, attrs, is_static, is_public }
    }

    fn prop_decl(&self, n: Node<'_>) -> Option<RawProp> {
        let mut attrs = Vec::new();
        let mut modifiers = Vec::new();
        let mut getter = false;
        let mut setter = false;
        let mut explicit_interface = false;
        let mut c = n.walk();
        for ch in n.children(&mut c) {
            match ch.kind() {
                "attribute_list" => attrs.extend(self.attrs(ch)),
                "modifier" => modifiers.push(self.text(ch).to_owned()),
                "explicit_interface_specifier" => explicit_interface = true,
                "arrow_expression_clause" => getter = true,
                "accessor_list" => {
                    let mut c2 = ch.walk();
                    for acc in ch.named_children(&mut c2) {
                        if acc.kind() != "accessor_declaration" {
                            continue;
                        }
                        let mut c3 = acc.walk();
                        for k in acc.children(&mut c3) {
                            match k.kind() {
                                "get" => getter = true,
                                "set" | "init" => setter = true,
                                _ => {}
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        let name = n.child_by_field_name("name").map(|x| self.text(x).to_owned())?;
        let ty = self.type_expr(n.child_by_field_name("type")?);
        Some(RawProp { name, ty, modifiers, attrs, getter, setter, explicit_interface })
    }

    fn attrs(&self, list: Node<'_>) -> Vec<Attr> {
        let mut c = list.walk();
        let attrs: Vec<Node<'_>> =
            list.named_children(&mut c).filter(|x| x.kind() == "attribute").collect();
        attrs
            .into_iter()
            .map(move |a| {
                let raw = a.child_by_field_name("name").map_or("", |x| self.text(x));
                let last = raw.rsplit(['.', ':']).next().unwrap_or(raw);
                let last = last.split('<').next().unwrap_or(last);
                let name = last.strip_suffix("Attribute").unwrap_or(last).to_owned();
                let mut args = Vec::new();
                let mut c2 = a.walk();
                for al in a.named_children(&mut c2) {
                    if al.kind() != "attribute_argument_list" {
                        continue;
                    }
                    let mut c3 = al.walk();
                    for arg in al.named_children(&mut c3) {
                        if arg.kind() != "attribute_argument" {
                            continue;
                        }
                        let n = arg.child_by_field_name("name").map(|x| self.text(x).to_owned());
                        // The value is the last named child that isn't the name.
                        let mut c4 = arg.walk();
                        let val = arg
                            .named_children(&mut c4)
                            .filter(|x| {
                                Some(x.id()) != arg.child_by_field_name("name").map(|y| y.id())
                            })
                            .filter(|x| !matches!(x.kind(), "name_equals" | "name_colon"))
                            .last();
                        if let Some(v) = val {
                            args.push((n, self.expr(v)));
                        }
                    }
                }
                Attr { name, args }
            })
            .collect()
    }

    fn field_decl(&self, n: Node<'_>) -> Vec<RawField> {
        let mut attrs = Vec::new();
        let mut mods = Vec::new();
        let mut decl = None;
        let mut c = n.walk();
        for ch in n.children(&mut c) {
            match ch.kind() {
                "attribute_list" => attrs.extend(self.attrs(ch)),
                "modifier" => mods.push(self.text(ch).to_owned()),
                "variable_declaration" => decl = Some(ch),
                _ => {}
            }
        }
        let Some(decl) = decl else { return Vec::new() };
        let Some(ty) = decl.child_by_field_name("type") else { return Vec::new() };
        let ty = self.type_expr(ty);
        let mut out = Vec::new();
        let mut c = decl.walk();
        for v in decl.named_children(&mut c) {
            if v.kind() != "variable_declarator" {
                continue;
            }
            let name = v.child_by_field_name("name").map_or("", |x| self.text(x)).to_owned();
            // The initializer is the named child after `=`.
            let mut init = None;
            let mut seen_eq = false;
            let mut c2 = v.walk();
            for ch in v.children(&mut c2) {
                if ch.kind() == "=" {
                    seen_eq = true;
                } else if seen_eq && ch.is_named() {
                    let e = if ch.kind() == "equals_value_clause" {
                        ch.named_child(0).map_or(Expr::Other(String::new()), |x| self.expr(x))
                    } else {
                        self.expr(ch)
                    };
                    init = Some(e);
                    break;
                } else if ch.kind() == "equals_value_clause" {
                    init = ch.named_child(0).map(|x| self.expr(x));
                    break;
                }
            }
            out.push(RawField {
                name,
                ty: ty.clone(),
                modifiers: mods.clone(),
                attrs: attrs.clone(),
                init,
            });
        }
        out
    }

    fn type_expr(&self, n: Node<'_>) -> TypeExpr {
        match n.kind() {
            "predefined_type" => TypeExpr::Predefined(self.text(n).to_owned()),
            "identifier" => {
                TypeExpr::Name { global: false, segments: vec![(self.text(n).to_owned(), vec![])] }
            }
            "generic_name" => {
                let name = n.named_child(0).map_or("", |x| self.text(x)).to_owned();
                let args = n
                    .named_child(1)
                    .map(|al| {
                        let mut c = al.walk();
                        al.named_children(&mut c).map(|a| self.type_expr(a)).collect()
                    })
                    .unwrap_or_default();
                TypeExpr::Name { global: false, segments: vec![(name, args)] }
            }
            "qualified_name" => {
                let q = n.child_by_field_name("qualifier");
                let nm = n.child_by_field_name("name");
                let (mut global, mut segs) = match q.map(|q| self.type_expr(q)) {
                    Some(TypeExpr::Name { global, segments }) => (global, segments),
                    _ => (false, Vec::new()),
                };
                if let Some(TypeExpr::Name { global: g2, segments: s2 }) =
                    nm.map(|x| self.type_expr(x))
                {
                    global |= g2;
                    segs.extend(s2);
                }
                TypeExpr::Name { global, segments: segs }
            }
            "alias_qualified_name" => {
                let alias = n.child_by_field_name("alias").map_or("", |x| self.text(x));
                let inner = n.child_by_field_name("name").map(|x| self.type_expr(x));
                match inner {
                    Some(TypeExpr::Name { segments, .. }) => {
                        TypeExpr::Name { global: alias == "global", segments }
                    }
                    _ => TypeExpr::Other(self.text(n).to_owned()),
                }
            }
            "array_type" => {
                let elem = n.child_by_field_name("type").map(|x| self.type_expr(x));
                let rank = n.child_by_field_name("rank").map_or(1, |r| {
                    1 + u8::try_from(self.text(r).matches(',').count()).unwrap_or(0)
                });
                match elem {
                    Some(e) => TypeExpr::Array(Box::new(e), rank),
                    None => TypeExpr::Other(self.text(n).to_owned()),
                }
            }
            "nullable_type" => match n.child_by_field_name("type").or_else(|| n.named_child(0)) {
                Some(e) => TypeExpr::Nullable(Box::new(self.type_expr(e))),
                None => TypeExpr::Other(self.text(n).to_owned()),
            },
            _ => TypeExpr::Other(self.text(n).to_owned()),
        }
    }

    fn expr(&self, n: Node<'_>) -> Expr {
        let text = self.text(n);
        match n.kind() {
            "integer_literal" => {
                let lower = text.to_ascii_lowercase();
                let suffix = (
                    lower.ends_with('u') || lower.ends_with("ul") || lower.ends_with("lu"),
                    lower.ends_with('l') || lower.ends_with("lu"),
                );
                parse_int(text).map_or_else(|| Expr::Other(text.into()), |v| Expr::Int(v, suffix))
            }
            "real_literal" => {
                let t = text.trim_end_matches(['f', 'F', 'd', 'D', 'm', 'M']).replace('_', "");
                t.parse().map_or_else(|_| Expr::Other(text.into()), Expr::Float)
            }
            "boolean_literal" => Expr::Bool(text == "true"),
            "null_literal" => Expr::Null,
            "string_literal" | "verbatim_string_literal" | "raw_string_literal" => {
                Expr::Str(unescape_string(text))
            }
            "character_literal" => {
                let inner = text.trim_matches('\'');
                let s = unescape_string(&format!("\"{inner}\""));
                s.chars().next().map_or(Expr::Other(text.into()), |c| Expr::Char(c as u32))
            }
            "identifier" => Expr::Ident(vec![text.to_owned()]),
            "member_access_expression" => {
                let mut parts = Vec::new();
                if let Some(e) = n.child_by_field_name("expression") {
                    match self.expr(e) {
                        Expr::Ident(p) => parts.extend(p),
                        _ => return Expr::Other(text.into()),
                    }
                }
                if let Some(nm) = n.child_by_field_name("name") {
                    parts.push(self.text(nm).to_owned());
                }
                Expr::Ident(parts)
            }
            "predefined_type" => Expr::Ident(vec![text.to_owned()]),
            "parenthesized_expression" => {
                n.named_child(0).map_or_else(|| Expr::Other(text.into()), |x| self.expr(x))
            }
            "prefix_unary_expression" => {
                let op = n.child(0).map_or("", |x| self.text(x)).to_owned();
                match n.named_child(0) {
                    Some(e) => Expr::Unary(op, Box::new(self.expr(e))),
                    None => Expr::Other(text.into()),
                }
            }
            "binary_expression" => {
                let l = n.child_by_field_name("left").map(|x| self.expr(x));
                let r = n.child_by_field_name("right").map(|x| self.expr(x));
                let op = n.child_by_field_name("operator").map_or("", |x| self.text(x));
                match (l, r) {
                    (Some(l), Some(r)) => Expr::Binary(op.to_owned(), Box::new(l), Box::new(r)),
                    _ => Expr::Other(text.into()),
                }
            }
            "cast_expression" => {
                let t = n.child_by_field_name("type").map(|x| self.type_expr(x));
                let v = n.child_by_field_name("value").map(|x| self.expr(x));
                match (t, v) {
                    (Some(t), Some(v)) => Expr::Cast(t, Box::new(v)),
                    _ => Expr::Other(text.into()),
                }
            }
            "default_expression" => Expr::Default,
            "object_creation_expression" => {
                let args_empty =
                    n.child_by_field_name("arguments").is_none_or(|a| a.named_child_count() == 0);
                let init = n.child_by_field_name("initializer");
                match (n.child_by_field_name("type"), args_empty, init) {
                    (Some(t), true, None) => Expr::NewEmpty(self.type_expr(t)),
                    (Some(t), false, None) => {
                        let args = n.child_by_field_name("arguments").map(|al| {
                            let mut c = al.walk();
                            al.named_children(&mut c)
                                .filter(|a| a.kind() == "argument")
                                .map(|a| {
                                    a.named_child(0)
                                        .map_or(Expr::Other(String::new()), |x| self.expr(x))
                                })
                                .collect::<Vec<_>>()
                        });
                        Expr::New(self.type_expr(t), args.unwrap_or_default())
                    }
                    _ => Expr::Other(text.into()),
                }
            }
            _ => Expr::Other(text.into()),
        }
    }
}

fn join_ns(a: &str, b: &str) -> String {
    if a.is_empty() { b.to_owned() } else { format!("{a}.{b}") }
}

fn parse_int(t: &str) -> Option<i128> {
    let t = t.replace('_', "");
    let t = t.trim_end_matches(['u', 'U', 'l', 'L']);
    if let Some(h) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
        i128::from_str_radix(h, 16).ok()
    } else if let Some(b) = t.strip_prefix("0b").or_else(|| t.strip_prefix("0B")) {
        i128::from_str_radix(b, 2).ok()
    } else {
        t.parse().ok()
    }
}

/// Decodes a C# string literal (regular or verbatim) to its value.
fn unescape_string(lit: &str) -> String {
    if let Some(v) = lit.strip_prefix('@') {
        let inner = v.strip_prefix('"').and_then(|s| s.strip_suffix('"')).unwrap_or(v);
        return inner.replace("\"\"", "\"");
    }
    let inner = lit.strip_prefix('"').and_then(|s| s.strip_suffix('"')).unwrap_or(lit);
    let mut out = String::with_capacity(inner.len());
    let mut it = inner.chars();
    while let Some(c) = it.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match it.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some('0') => out.push('\0'),
            Some('a') => out.push('\u{7}'),
            Some('b') => out.push('\u{8}'),
            Some('f') => out.push('\u{c}'),
            Some('v') => out.push('\u{b}'),
            Some('u') => {
                let h: String = it.by_ref().take(4).collect();
                if let Some(ch) = u32::from_str_radix(&h, 16).ok().and_then(char::from_u32) {
                    out.push(ch);
                }
            }
            Some('x') => {
                // `\x` takes one to four hex digits; ILSpy emits `\u` instead, so this is rare.
                let h: String = it.clone().take_while(char::is_ascii_hexdigit).take(4).collect();
                for _ in 0..h.len() {
                    it.next();
                }
                if let Some(ch) = u32::from_str_radix(&h, 16).ok().and_then(char::from_u32) {
                    out.push(ch);
                }
            }
            Some(o) => out.push(o),
            None => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str = r#"
using System;
using Foo = Bar.Baz;
namespace A.B;
[Serializable]
[TypeId("0b3cc43201601904bb7eb333c9b646ff")]
public class C<T> : Base<T>, IFoo where T : class
{
    [Flags] private enum E : byte { X = 1, Y = 1 << 2, Z }
    [SerializeField][JsonProperty(PropertyName = "PrototypeLink", DefaultValueHandling = DefaultValueHandling.Ignore)]
    private List<string> m_Overrides = new List<string>(), m_B;
    public static readonly int[] S = new int[0];
    public float F = 1f, G = -2.5f;
    public global::X.Y Q;
    public override void OnEnable() {}
    public C() { F = 2f; }
    public struct S2 { public int x; }
}
"#;

    #[test]
    fn declarations() {
        let asm: Arc<str> = Arc::from("Asm");
        let types = parse_file(&asm, "a.cs", SRC).unwrap();
        let names: Vec<&str> = types.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, ["C", "E", "S2"]);
        let c = &types[0];
        assert_eq!(c.namespace, "A.B");
        assert_eq!(c.generics, ["T"]);
        assert_eq!(c.bases.len(), 2);
        assert!(c.has_attr("Serializable"));
        let tid = c.attrs.iter().find(|a| a.name == "TypeId").unwrap();
        assert_eq!(tid.first_positional_string(), Some("0b3cc43201601904bb7eb333c9b646ff"));
        assert_eq!(c.usings.aliases[0].0, "Foo");
        let f: Vec<&str> = c.fields.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(f, ["m_Overrides", "m_B", "S", "F", "G", "Q"]);
        let jp = c.fields[0].attrs.iter().find(|a| a.name == "JsonProperty").unwrap();
        assert_eq!(jp.string_arg("PropertyName"), Some("PrototypeLink"));
        assert!(matches!(c.fields[0].init, Some(Expr::NewEmpty(_))));
        assert_eq!(c.fields[4].init, Some(Expr::Unary("-".into(), Box::new(Expr::Float(2.5)))));
        assert_eq!(c.fields[5].ty.text(), "global::X.Y");
        assert!(c.ctor_with_body);
        assert_eq!(c.methods[0].name, "OnEnable");
        let e = &types[1];
        assert_eq!(e.outer, Some(0));
        assert_eq!(e.enum_members.len(), 3);
        assert!(matches!(e.enum_members[1].1, Some(Expr::Binary(..))));
    }

    #[test]
    fn properties_and_ctors() {
        let src = r#"
namespace N;
public class P : Base {
    [JsonProperty] public int A { get; private set; }
    [JsonProperty(PropertyName = "m_B")] private List<int> B { set { } }
    public int C => 1;
    public int D { get; }
    int I.E { get; set; }
    public string F { get; init; } = "x";
    [JsonConstructor] public P(EntityDataBase owner) { }
    protected P(JsonConstructorMark _) : base(_) { }
    static P() { }
}
"#;
        let asm: Arc<str> = Arc::from("Asm");
        let types = parse_file(&asm, "p.cs", src).unwrap();
        let p = &types[0];
        let props: Vec<(&str, bool, bool, bool)> = p
            .props
            .iter()
            .map(|x| (x.name.as_str(), x.getter, x.setter, x.explicit_interface))
            .collect();
        assert_eq!(
            props,
            [
                ("A", true, true, false),
                ("B", false, true, false),
                ("C", true, false, false),
                ("D", true, false, false),
                ("E", true, true, true),
                ("F", true, true, false),
            ]
        );
        assert_eq!(p.props[1].attrs[0].string_arg("PropertyName"), Some("m_B"));
        assert_eq!(p.ctors.len(), 3);
        assert_eq!(p.ctors[0].attrs, ["JsonConstructor"]);
        assert_eq!(p.ctors[0].params[0].text(), "EntityDataBase");
        assert_eq!(p.ctors[0].param_names, ["owner"]);
        assert!(p.ctors[0].is_public && !p.ctors[1].is_public);
        assert_eq!(p.ctors[1].params[0].text(), "JsonConstructorMark");
        assert!(p.ctors[2].is_static);
    }

    #[test]
    fn strings() {
        assert_eq!(unescape_string(r#""a\"bA""#), "a\"bA");
        assert_eq!(unescape_string(r#"@"a""b""#), "a\"b");
    }
}
