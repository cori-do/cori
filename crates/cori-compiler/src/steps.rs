// SPDX-License-Identifier: Apache-2.0
//! Static extraction of a step file with oxc.
//!
//! A step file is `export default step.<kind>({ ... })`. We extract literals,
//! nested steps, goto targets, and the static references of every binding
//! function (`input`, `if`, `on`, `over`, `until`). Bindings may only use
//! static member access on the context parameter; anything dynamic or any
//! escape of the whole context is a compile error.
use oxc_allocator::Allocator;
use oxc_ast::ast::*;
use oxc_ast_visit::{Visit, walk};
use oxc_parser::Parser;
use oxc_span::{GetSpan, SourceType};
use serde::Serialize;
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Serialize)]
pub struct Binding {
    pub field: String,
    pub refs: BTreeSet<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct StepDecl {
    pub name: String,
    pub kind: String,
    pub description: String,
    pub file: String,
    /// Position in the manifest's `steps:` list (0-based); nested steps inherit the parent's.
    pub index: usize,
    pub nested_in: Option<String>,
    /// Slot inside the parent: then | else | each | body | on_error | default | cases.<label>
    pub slot: Option<String>,
    pub literals: Map<String, Value>,
    pub bindings: Vec<Binding>,
    /// Union of all binding refs.
    pub refs: BTreeSet<String>,
    pub nested: Vec<StepDecl>,
    pub goto_targets: Vec<(String, String)>,
    pub line: usize,
}

impl StepDecl {
    pub fn depends_on(&self) -> BTreeSet<String> {
        let mut out = BTreeSet::new();
        for r in &self.refs {
            if let Some(rest) = r.strip_prefix("steps.") {
                let name = rest.split(['.', '[']).next().unwrap_or(rest);
                out.insert(name.to_string());
            }
        }
        out
    }
}

pub struct Parsed {
    pub decl: StepDecl,
    pub errors: Vec<String>,
    /// Non-fatal findings (a non-deterministic value in a binding).
    pub warnings: Vec<String>,
}

pub fn parse_step_file(file: &str, src: &str, index: usize) -> Result<Parsed, Vec<String>> {
    let alloc = Allocator::default();
    let ret = Parser::new(&alloc, src, SourceType::ts()).parse();
    if !ret.diagnostics.is_empty() {
        return Err(ret.diagnostics.iter().map(|e| format!("{file}: parse error: {e}")).collect());
    }
    let mut has_sdk_import = false;
    let mut default_call: Option<&CallExpression> = None;
    for stmt in ret.program.body.iter() {
        match stmt {
            Statement::ImportDeclaration(imp) => {
                if imp.source.value.as_str() == "@cori-do/sdk" { has_sdk_import = true; }
                if imp.source.value.as_str().starts_with("node:") {
                    return Err(vec![format!("{file}: `node:` imports are not allowed in step files")]);
                }
            }
            Statement::ExportDefaultDeclaration(d) => {
                if let ExportDefaultDeclarationKind::CallExpression(c) = &d.declaration { default_call = Some(c); }
            }
            _ => {}
        }
    }
    if !has_sdk_import { return Err(vec![format!("{file}: missing `import {{ step }} from \"@cori-do/sdk\"`")]); }
    let call = default_call.ok_or_else(|| vec![format!("{file}: expected `export default step.<kind>({{ ... }})`")])?;
    let mut cx = Cx { file: file.to_string(), src, errors: Vec::new(), warnings: Vec::new(), index };
    let decl = cx.parse_step_call(call, None, None).ok_or_else(|| cx.errors.clone())?;
    if cx.errors.iter().any(|e| e.contains("[fatal]")) { return Err(cx.errors); }
    // the file is named after the step: steps/<name>.ts declares `name: "<name>"`
    let stem = std::path::Path::new(file).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    if decl.name != stem { cx.errors.push(format!("{file}:{}: step name `{}` must equal the file name (rename the file to steps/{}.ts or the step to `{stem}`)", decl.line, decl.name, decl.name)); }
    Ok(Parsed { decl, errors: cx.errors, warnings: cx.warnings })
}

struct Cx<'s> {
    file: String,
    src: &'s str,
    errors: Vec<String>,
    warnings: Vec<String>,
    index: usize,
}

const BINDING_FIELDS: &[&str] = &["input", "if", "on", "over", "until"];
const NESTED_SLOTS: &[&str] = &["then", "else", "each", "body", "on_error", "default"];

impl<'s> Cx<'s> {
    fn line_of(&self, span: oxc_span::Span) -> usize {
        self.src[..span.start as usize].matches('\n').count() + 1
    }
    fn err(&mut self, span: oxc_span::Span, msg: impl AsRef<str>) {
        let line = self.line_of(span);
        self.errors.push(format!("{}:{}: {}", self.file, line, msg.as_ref()));
    }

    fn step_kind_of(&self, call: &CallExpression) -> Option<String> {
        if let Expression::StaticMemberExpression(m) = &call.callee {
            if let Expression::Identifier(id) = &m.object {
                if id.name.as_str() == "step" { return Some(m.property.name.as_str().to_string()); }
            }
        }
        None
    }

    fn parse_step_call(&mut self, call: &CallExpression, parent: Option<&str>, slot: Option<&str>) -> Option<StepDecl> {
        let kind = match self.step_kind_of(call) {
            Some(k) => k,
            None => { self.err(call.span, "[fatal] callee must be `step.<kind>`"); return None; }
        };
        let obj = match call.arguments.first() {
            Some(Argument::ObjectExpression(o)) => o,
            _ => { self.err(call.span, "[fatal] `step.<kind>(...)` takes one object literal"); return None; }
        };
        let mut decl = StepDecl {
            name: String::new(), kind: kind.clone(), description: String::new(), file: self.file.clone(),
            index: self.index, nested_in: parent.map(|s| s.to_string()), slot: slot.map(|s| s.to_string()),
            literals: Map::new(), bindings: Vec::new(), refs: BTreeSet::new(), nested: Vec::new(), goto_targets: Vec::new(),
            line: self.line_of(call.span),
        };
        for prop in obj.properties.iter() {
            let ObjectPropertyKind::ObjectProperty(p) = prop else { self.err(obj.span, "[fatal] spread in step options is not allowed"); return None; };
            let key = match &p.key {
                PropertyKey::StaticIdentifier(id) => id.name.as_str().to_string(),
                PropertyKey::StringLiteral(s) => s.value.as_str().to_string(),
                _ => { self.err(p.span, "[fatal] computed option keys are not allowed"); return None; }
            };
            self.parse_option(&mut decl, &key, &p.value);
        }
        if decl.name.is_empty() { self.err(call.span, "[fatal] step `name` is required (a string literal)"); return None; }
        if !decl.name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_') || !decl.name.chars().next().unwrap().is_ascii_lowercase() {
            self.err(call.span, format!("[fatal] step name `{}` must be snake_case", decl.name));
        }
        for b in &decl.bindings { decl.refs.extend(b.refs.iter().cloned()); }
        Some(decl)
    }

    fn parse_option(&mut self, decl: &mut StepDecl, key: &str, value: &Expression) {
        match key {
            "name" => { if let Some(s) = str_lit(value) { decl.name = s; } else { self.err(value.span(), "[fatal] `name` must be a string literal"); } }
            "description" => { if let Some(s) = str_lit(value) { decl.description = s; } }
            k if BINDING_FIELDS.contains(&k) => {
                let (refs, errs) = self.analyze_binding(value);
                for e in errs { self.errors.push(e); }
                decl.bindings.push(Binding { field: k.to_string(), refs });
            }
            "request" => {
                let lits = LiteralFinder::run(value);
                if let Some(m) = lits.method { decl.literals.insert("method".into(), Value::String(m)); }
                else { self.err(value.span(), "`request` must return an object literal with a literal `method`"); }
            }
            "command" => {
                let lits = LiteralFinder::run(value);
                if let Some(a) = lits.argv0 { decl.literals.insert("argv0".into(), Value::String(a)); }
                else { self.err(value.span(), "`command` must return an array literal whose first element is a string literal (the binary)"); }
            }
            "cases" => {
                if let Expression::ObjectExpression(o) = value {
                    for prop in o.properties.iter() {
                        if let ObjectPropertyKind::ObjectProperty(p) = prop {
                            let label = match &p.key { PropertyKey::StaticIdentifier(id) => id.name.as_str().to_string(), PropertyKey::StringLiteral(s) => s.value.as_str().to_string(), _ => continue };
                            self.parse_path(decl, &format!("cases.{label}"), &p.value);
                        }
                    }
                } else { self.err(value.span(), "[fatal] `cases` must be an object literal"); }
            }
            k if NESTED_SLOTS.contains(&k) => self.parse_path(decl, k, value),
            // `files`: the files an `llm` step's model reads, a function of the step's input like `prompt`
            "output" | "prompt" | "run" | "parse" | "examples" | "files" => { decl.literals.insert(format!("has_{key}"), Value::Bool(true)); }
            // a store query: the object `{ text, params }`, or the function that binds its params and returns one, whose
            // text the checks read when it is written out (no `${…}`)
            "query" if expr_to_json(value).is_none() => {
                let text = returned_query_text(value);
                decl.literals.insert(key.to_string(), text.map(|t| serde_json::json!({ "text": t })).unwrap_or_else(|| Value::String("<expr>".into())));
            }
            _ => {
                if let Some(v) = expr_to_json(value) { decl.literals.insert(key.to_string(), v); }
                else { decl.literals.insert(key.to_string(), Value::String("<expr>".into())); }
            }
        }
    }

    fn parse_path(&mut self, decl: &mut StepDecl, slot: &str, value: &Expression) {
        match value {
            Expression::CallExpression(c) => {
                if let Expression::Identifier(id) = &c.callee {
                    if id.name.as_str() == "goto" {
                        match c.arguments.first().and_then(|a| if let Argument::StringLiteral(s) = a { Some(s.value.as_str().to_string()) } else { None }) {
                            Some(t) => decl.goto_targets.push((slot.to_string(), t)),
                            None => self.err(c.span, "[fatal] `goto` takes a string literal step name"),
                        }
                        return;
                    }
                }
                if self.step_kind_of(c).is_some() {
                    let parent = decl.name.clone();
                    if let Some(n) = self.parse_step_call(c, Some(&parent), Some(slot)) {
                        if crate::is_container_kind(&n.kind) {
                            self.err(c.span, "[fatal] builtins cannot nest builtins");
                        }
                        decl.nested.push(n);
                    }
                    return;
                }
                self.err(c.span, format!("[fatal] `{slot}` must be an inline `step.<kind>({{...}})` or `goto(\"name\")`"));
            }
            _ => self.err(value.span(), format!("[fatal] `{slot}` must be an inline `step.<kind>({{...}})` or `goto(\"name\")`")),
        }
    }

    /// Returns the static refs of a binding function and the errors found.
    fn analyze_binding(&mut self, value: &Expression) -> (BTreeSet<String>, Vec<String>) {
        let (params, body): (&FormalParameters, BodyRef) = match value {
            Expression::ArrowFunctionExpression(f) => (&f.params, BodyRef::Arrow(&f.body)),
            Expression::FunctionExpression(f) => match &f.body { Some(b) => (&f.params, BodyRef::Block(b)), None => return (BTreeSet::new(), vec![format!("{}: binding has no body", self.file)]) },
            _ => return (BTreeSet::new(), vec![format!("{}:{}: bindings must be inline arrow functions", self.file, self.line_of(value.span()))]),
        };
        let mut roots: BTreeMap<String, String> = BTreeMap::new();
        if let Some(p0) = params.items.first() {
            collect_roots(&p0.pattern, "", &mut roots);
        }
        let mut v = RefVisitor { roots, refs: BTreeSet::new(), errors: Vec::new(), warnings: Vec::new(), file: self.file.clone(), src: self.src };
        match body {
            BodyRef::Arrow(b) => match b {
                ArrowFunctionBody::FunctionBody(fb) => v.visit_function_body(fb),
                other => { if let Some(e) = other.as_expression() { v.visit_expression(e); } }
            },
            BodyRef::Block(b) => v.visit_function_body(b),
        }
        self.warnings.extend(v.warnings);
        (v.refs, v.errors)
    }
}

enum BodyRef<'a, 'b> { Arrow(&'b ArrowFunctionBody<'a>), Block(&'b FunctionBody<'a>) }

/// The `text` a query function returns, when it is written out (no `${…}`): `(x) => ({ text: "…", params: [x] })`, or a
/// body whose top-level `return` gives such an object. Only the returned object's own `text` counts.
fn returned_query_text(value: &Expression) -> Option<String> {
    fn object_text(e: &Expression) -> Option<String> {
        match e {
            Expression::ParenthesizedExpression(p) => object_text(&p.expression),
            Expression::ObjectExpression(o) => o.properties.iter().find_map(|p| match p {
                ObjectPropertyKind::ObjectProperty(p) if matches!(&p.key, PropertyKey::StaticIdentifier(id) if id.name.as_str() == "text") => str_lit(&p.value),
                _ => None,
            }),
            _ => None,
        }
    }
    fn returned(body: &FunctionBody) -> Option<String> {
        body.statements.iter().find_map(|s| match s { Statement::ReturnStatement(r) => r.argument.as_ref().and_then(object_text), _ => None })
    }
    match value {
        Expression::ArrowFunctionExpression(f) => match &f.body {
            ArrowFunctionBody::FunctionBody(fb) => returned(fb),
            other => other.as_expression().and_then(object_text),
        },
        Expression::FunctionExpression(f) => f.body.as_ref().and_then(|b| returned(b)),
        _ => None,
    }
}

fn collect_roots(pat: &BindingPattern, prefix: &str, roots: &mut BTreeMap<String, String>) {
    match pat {
        BindingPattern::BindingIdentifier(id) => { roots.insert(id.name.as_str().to_string(), prefix.to_string()); }
        BindingPattern::ObjectPattern(o) => {
            for p in o.properties.iter() {
                let key = match &p.key { PropertyKey::StaticIdentifier(id) => id.name.as_str().to_string(), PropertyKey::StringLiteral(s) => s.value.as_str().to_string(), _ => continue };
                let sub = if prefix.is_empty() { key } else { format!("{prefix}.{key}") };
                collect_roots(&p.value, &sub, roots);
            }
        }
        BindingPattern::AssignmentPattern(a) => collect_roots(&a.left, prefix, roots),
        BindingPattern::ArrayPattern(_) => {}
    }
}

struct RefVisitor<'s> {
    roots: BTreeMap<String, String>,
    refs: BTreeSet<String>,
    errors: Vec<String>,
    /// Bindings are evaluated in workflow code, which is replayed: a clock or a random value differs on replay.
    warnings: Vec<String>,
    file: String,
    src: &'s str,
}

impl<'s> RefVisitor<'s> {
    fn line_of(&self, span: oxc_span::Span) -> usize { self.src[..span.start as usize].matches('\n').count() + 1 }
    fn text(&self, span: oxc_span::Span) -> &str { &self.src[span.start as usize..span.end as usize] }
    /// Unroll `root.a.b.c` into a chain rooted at a context alias.
    fn chain(&self, e: &Expression) -> Option<Vec<String>> {
        match e {
            Expression::Identifier(id) => self.roots.get(id.name.as_str()).map(|prefix| {
                if prefix.is_empty() { vec![] } else { prefix.split('.').map(|s| s.to_string()).collect() }
            }),
            Expression::StaticMemberExpression(m) => { let mut c = self.chain(&m.object)?; c.push(m.property.name.as_str().to_string()); Some(c) }
            Expression::ComputedMemberExpression(m) => {
                let mut c = self.chain(&m.object)?;
                match &m.expression {
                    Expression::NumericLiteral(n) => { c.push(format!("[{}]", n.value)); Some(c) }
                    Expression::StringLiteral(s) => { c.push(s.value.as_str().to_string()); Some(c) }
                    _ => { c.push("[dynamic]".into()); Some(c) }
                }
            }
            Expression::ChainExpression(ch) => match &ch.expression {
                ChainElement::StaticMemberExpression(m) => { let mut c = self.chain(&m.object)?; c.push(m.property.name.as_str().to_string()); Some(c) }
                ChainElement::ComputedMemberExpression(m) => { let mut c = self.chain(&m.object)?; c.push("[dynamic]".into()); Some(c) }
                _ => None,
            },
            Expression::ParenthesizedExpression(p) => self.chain(&p.expression),
            Expression::TSNonNullExpression(t) => self.chain(&t.expression),
            Expression::TSAsExpression(t) => self.chain(&t.expression),
            Expression::TSSatisfiesExpression(t) => self.chain(&t.expression),
            _ => None,
        }
    }
    fn record(&mut self, chain: Vec<String>, span: oxc_span::Span) {
        if chain.is_empty() { self.errors.push(format!("{}:{}: the whole context escapes the binding (`{}`); reference fields explicitly", self.file, self.line_of(span), self.text(span))); return; }
        if chain[0] == "steps" && chain.len() == 1 { self.errors.push(format!("{}:{}: `steps` as a whole escapes the binding; reference `steps.<name>...`", self.file, self.line_of(span))); return; }
        if chain.iter().any(|c| c == "[dynamic]") { self.errors.push(format!("{}:{}: dynamic access on the context (`{}`) cannot be proven; use static member access", self.file, self.line_of(span), self.text(span))); return; }
        if !matches!(chain[0].as_str(), "steps" | "params" | "user" | "run" | "item") { self.errors.push(format!("{}:{}: unknown context member `{}`", self.file, self.line_of(span), chain[0])); return; }
        self.refs.insert(chain.join(".").replace(".[", "["));
    }
}

impl<'s> RefVisitor<'s> {
    fn chain_static(&self, m: &StaticMemberExpression) -> Option<Vec<String>> {
        let mut c = self.chain(&m.object)?; c.push(m.property.name.as_str().to_string()); Some(c)
    }
    fn chain_computed(&self, m: &ComputedMemberExpression) -> Option<Vec<String>> {
        let mut c = self.chain(&m.object)?;
        match &m.expression {
            Expression::NumericLiteral(n) => c.push(format!("[{}]", n.value)),
            Expression::StringLiteral(s) => c.push(s.value.as_str().to_string()),
            _ => c.push("[dynamic]".into()),
        }
        Some(c)
    }
}

impl<'s> RefVisitor<'s> {
    fn non_deterministic(&mut self, span: oxc_span::Span) {
        self.warnings.push(format!("{}:{}: non-deterministic value in a binding (`{}`): bindings are replayed, use ctx.run.started_at / ctx.run.id (or compute it in a step)", self.file, self.line_of(span), self.text(span)));
    }
}

/// `Date.now()`, `Math.random()`, `crypto.randomUUID()`, `performance.now()` (also `globalThis.crypto.randomUUID()`).
fn is_non_deterministic_call(c: &CallExpression) -> bool {
    let Expression::StaticMemberExpression(m) = &c.callee else { return false };
    let object = match &m.object {
        Expression::Identifier(id) => id.name.as_str(),
        Expression::StaticMemberExpression(g) if matches!(&g.object, Expression::Identifier(id) if matches!(id.name.as_str(), "globalThis" | "window" | "self")) => g.property.name.as_str(),
        _ => return false,
    };
    matches!((object, m.property.name.as_str()), ("Date", "now") | ("Math", "random") | ("crypto", "randomUUID") | ("crypto", "getRandomValues") | ("performance", "now"))
}

impl<'a, 's> Visit<'a> for RefVisitor<'s> {
    fn visit_new_expression(&mut self, it: &NewExpression<'a>) {
        // `new Date()` without arguments reads the clock; `new Date(ctx.run.started_at)` is fine
        if it.arguments.is_empty() && matches!(&it.callee, Expression::Identifier(id) if id.name.as_str() == "Date") { self.non_deterministic(it.span); }
        walk::walk_new_expression(self, it);
    }
    fn visit_call_expression(&mut self, it: &CallExpression<'a>) {
        if is_non_deterministic_call(it) { self.non_deterministic(it.span); }
        // `Date()` called as a function returns the current time as a string
        if it.arguments.is_empty() && matches!(&it.callee, Expression::Identifier(id) if id.name.as_str() == "Date") { self.non_deterministic(it.span); }
        // `ctx.steps.x.rows.map(...)`: record the object chain, not the method name; still walk the arguments.
        if let Expression::StaticMemberExpression(m) = &it.callee {
            if let Some(c) = self.chain(&m.object) {
                self.record(c, m.span);
                for a in it.arguments.iter() { if let Some(e) = a.as_expression() { self.visit_expression(e); } }
                return;
            }
        }
        walk::walk_call_expression(self, it);
    }
    fn visit_static_member_expression(&mut self, it: &StaticMemberExpression<'a>) {
        if let Some(c) = self.chain_static(it) { self.record(c, it.span); return; }
        walk::walk_static_member_expression(self, it);
    }
    fn visit_computed_member_expression(&mut self, it: &ComputedMemberExpression<'a>) {
        if let Some(c) = self.chain_computed(it) {
            self.record(c, it.span);
            // the index expression may itself read the context
            self.visit_expression(&it.expression);
            return;
        }
        walk::walk_computed_member_expression(self, it);
    }
    fn visit_identifier_reference(&mut self, it: &IdentifierReference<'a>) {
        if let Some(prefix) = self.roots.get(it.name.as_str()).cloned() {
            let chain: Vec<String> = if prefix.is_empty() { vec![] } else { prefix.split('.').map(|s| s.to_string()).collect() };
            self.record(chain, it.span);
        }
    }
}

// ---------------------------------------------------------------------------
// literal helpers
// ---------------------------------------------------------------------------

pub(crate) fn str_lit(e: &Expression) -> Option<String> {
    match e {
        Expression::StringLiteral(s) => Some(s.value.as_str().to_string()),
        Expression::TemplateLiteral(t) if t.expressions.is_empty() => t.quasis.first().map(|q| q.value.raw.as_str().to_string()),
        Expression::ParenthesizedExpression(p) => str_lit(&p.expression),
        Expression::TSAsExpression(t) => str_lit(&t.expression),
        _ => None,
    }
}

/// A JS number as JSON: an integer when it has no fraction. The engine reads `timeout_ms`, `retries.max`,
/// `max_concurrency` and `max_iterations` as integers; stored as floats (`5000.0`) they were ignored.
fn number(v: f64) -> Option<Value> {
    if v.fract() == 0.0 && v.abs() <= 9_007_199_254_740_991.0 { Some(Value::from(v as i64)) } else { serde_json::Number::from_f64(v).map(Value::Number) }
}

pub(crate) fn expr_to_json(e: &Expression) -> Option<Value> {
    match e {
        Expression::StringLiteral(_) | Expression::TemplateLiteral(_) => str_lit(e).map(Value::String),
        Expression::NumericLiteral(n) => number(n.value),
        Expression::BooleanLiteral(b) => Some(Value::Bool(b.value)),
        Expression::NullLiteral(_) => Some(Value::Null),
        Expression::UnaryExpression(u) => match (&u.operator, &u.argument) {
            (oxc_ast::ast::UnaryOperator::UnaryNegation, Expression::NumericLiteral(n)) => number(-n.value),
            _ => None,
        },
        Expression::ArrayExpression(a) => {
            let mut out = Vec::new();
            for el in a.elements.iter() { out.push(expr_to_json(el.as_expression()?)?); }
            Some(Value::Array(out))
        }
        Expression::ObjectExpression(o) => {
            let mut map = Map::new();
            for prop in o.properties.iter() {
                let ObjectPropertyKind::ObjectProperty(p) = prop else { return None };
                let key = match &p.key { PropertyKey::StaticIdentifier(id) => id.name.as_str().to_string(), PropertyKey::StringLiteral(s) => s.value.as_str().to_string(), _ => return None };
                map.insert(key, expr_to_json(&p.value)?);
            }
            Some(Value::Object(map))
        }
        Expression::ParenthesizedExpression(p) => expr_to_json(&p.expression),
        Expression::TSAsExpression(t) => expr_to_json(&t.expression),
        Expression::TSSatisfiesExpression(t) => expr_to_json(&t.expression),
        _ => None,
    }
}

/// Finds literal facts inside a function: the `method` of a returned request object, the argv[0] of a returned command array.
#[derive(Default)]
struct LiteralFinder { method: Option<String>, argv0: Option<String> }

impl LiteralFinder {
    fn run(e: &Expression) -> Self { let mut f = Self::default(); f.visit_expression(e); f }
}

impl<'a> Visit<'a> for LiteralFinder {
    fn visit_object_property(&mut self, it: &ObjectProperty<'a>) {
        if let PropertyKey::StaticIdentifier(id) = &it.key {
            if id.name.as_str() == "method" && self.method.is_none() { self.method = str_lit(&it.value); }
        }
        walk::walk_object_property(self, it);
    }
    fn visit_array_expression(&mut self, it: &ArrayExpression<'a>) {
        if self.argv0.is_none() {
            if let Some(first) = it.elements.first().and_then(|e| e.as_expression()) { self.argv0 = str_lit(first); }
        }
        walk::walk_array_expression(self, it);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn warnings_of(input: &str) -> Vec<String> {
        let src = format!("import {{ step }} from \"@cori-do/sdk\";\nexport default step.code({{ name: \"a\", input: {input}, run: () => ({{}}) }});\n");
        parse_step_file("steps/a.ts", &src, 0).map_err(|e| e.join("; ")).expect("parses").warnings
    }

    /// A store query's text is read from its function form too (the form that binds params), when it is written out.
    #[test]
    fn a_query_text_is_read_from_its_function() {
        let query = |q: &str| {
            let src = format!("import {{ step }} from \"@cori-do/sdk\";\nexport default step.store({{ name: \"q\", connector: \"s\", input: () => ({{ id: 1 }}), query: {q}, output: x }});\n");
            parse_step_file("steps/q.ts", &src, 0).map_err(|e| e.join("; ")).expect("parses").decl.literals["query"].clone()
        };
        assert_eq!(query("{ text: \"select 1\", params: [] }")["text"], "select 1");
        assert_eq!(query("({ id }) => ({ text: `update t set a = 1\n where id = $1`, params: [id] })")["text"], "update t set a = 1\n where id = $1");
        assert_eq!(query("function (x) { const p = [x]; return { text: \"select 2\", params: p }; }")["text"], "select 2");
        // the returned object's own text only, never one met on the way
        assert_eq!(query("({ id }) => ({ text: `select 3`, params: [{ text: \"not this\" }] })")["text"], "select 3");
        assert_eq!(query("(x) => { const draft = { text: \"update t set a = 1\" }; return build(draft, x); }"), "<expr>");
        // built when the step runs: no text to check
        assert_eq!(query("({ id }) => ({ text: `select ${cols} from t`, params: [id] })"), "<expr>");
        assert_eq!(query("buildQuery"), "<expr>");
    }

    #[test]
    fn whole_numbers_are_integers() {
        let src = "import { step } from \"@cori-do/sdk\";\nexport default step.map({ name: \"m\", over: (ctx) => ctx.params.ids, max_concurrency: 4, timeout_ms: 5_000, retries: { max: 2 }, ratio: 0.5, offset: -3, each: step.code({ name: \"e\", run: () => ({}) }) });\n";
        let lits = parse_step_file("steps/m.ts", src, 0).map_err(|e| e.join("; ")).expect("parses").decl.literals;
        assert_eq!(lits["max_concurrency"].as_u64(), Some(4));
        assert_eq!(lits["timeout_ms"].as_u64(), Some(5000));
        assert_eq!(lits["retries"]["max"].as_u64(), Some(2));
        assert_eq!(lits["offset"].as_i64(), Some(-3));
        assert_eq!(lits["ratio"].as_f64(), Some(0.5));
    }

    #[test]
    fn non_deterministic_values_in_bindings_are_flagged() {
        for bad in ["(ctx) => ({ at: new Date().toISOString() })", "(ctx) => ({ at: Date.now() })", "(ctx) => ({ n: Math.random() })", "(ctx) => ({ id: crypto.randomUUID() })", "(ctx) => ({ t: performance.now() })", "(ctx) => ({ id: globalThis.crypto.randomUUID() })"] {
            let w = warnings_of(bad);
            assert!(w.iter().any(|m| m.contains("non-deterministic value in a binding") && m.contains("ctx.run.started_at")), "{bad}: {w:?}");
        }
        for good in ["(ctx) => ({ at: ctx.run.started_at })", "(ctx) => ({ at: new Date(ctx.run.started_at).getTime() })", "(ctx) => ({ id: ctx.run.id })"] {
            assert!(warnings_of(good).is_empty(), "{good}");
        }
    }
}
