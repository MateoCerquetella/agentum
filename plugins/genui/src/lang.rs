//! The ```ui language agents write (BB's Inline UI), without any web dependency (so it is tested
//! natively; the web part draws what it evaluates to).
//!
//! One statement per line: `$x = 8` declares state that inputs bind to, `name = expr` names a value
//! (or a function: `f = (a, b) => …`), and each bare expression is drawn. A line that ends with an
//! operator, or a bracket left open, continues on the next. Expressions are JavaScript's: numbers,
//! strings, `templates ${x}`, [lists], {objects}, `...spread`, x.y, x[i], + - * / % **, comparisons,
//! && || ?? !, ?:, arrow functions for map/filter/find/reduce/sort, and round/fmt/sum/min/max/abs/
//! entries. A capitalised call is a component (`Card("Hi", [Text("…")])`) or an action
//! (`Send("go")`) and evaluates to a [`Node`]; named arguments go last (`Slider($n, 0, 10, label: "N")`).
//!
//! Nothing here panics on bad input: a line that doesn't parse, or a component whose arguments fail,
//! becomes an `Error` node with the reason, and the rest still draws.
use std::collections::BTreeMap;
use std::sync::Arc;

pub type R<T> = Result<T, String>;
/// `$name` → its value.
pub type State = BTreeMap<String, Val>;

// ───────────────────────────── values ─────────────────────────────

#[derive(Clone, Debug)]
pub enum Val {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    List(Vec<Val>),
    /// Keys in the order written.
    Obj(Vec<(String, Val)>),
    Node(Arc<Node>),
    Func(Arc<Func>),
}

/// A component or an action: `Name(args…, key: value…)`.
#[derive(Debug, PartialEq)]
pub struct Node {
    pub name: String,
    pub args: Vec<Val>,
    pub named: Vec<(String, Val)>,
    /// Per positional argument: the `$state` it was written as, for inputs and `Set`.
    pub binds: Vec<Option<String>>,
}

#[derive(Debug)]
pub struct Func {
    params: Vec<String>,
    body: Arc<Expr>,
    /// The arrow parameters in scope where it was written.
    env: Vec<(String, Val)>,
}

impl PartialEq for Val {
    fn eq(&self, o: &Val) -> bool {
        match (self, o) {
            (Val::Null, Val::Null) => true,
            (Val::Bool(a), Val::Bool(b)) => a == b,
            (Val::Num(a), Val::Num(b)) => a == b,
            (Val::Str(a), Val::Str(b)) => a == b,
            (Val::List(a), Val::List(b)) => a == b,
            (Val::Obj(a), Val::Obj(b)) => a.len() == b.len() && a.iter().all(|(k, v)| b.iter().any(|(k2, v2)| k == k2 && v == v2)),
            (Val::Node(a), Val::Node(b)) => Arc::ptr_eq(a, b) || a == b,
            (Val::Func(a), Val::Func(b)) => Arc::ptr_eq(a, b),
            _ => false,
        }
    }
}

impl Val {
    pub fn truthy(&self) -> bool {
        match self {
            Val::Null => false,
            Val::Bool(b) => *b,
            Val::Num(n) => *n != 0.0 && !n.is_nan(),
            Val::Str(s) => !s.is_empty(),
            _ => true,
        }
    }
    pub fn as_num(&self) -> Option<f64> {
        match self {
            Val::Num(n) => Some(*n),
            Val::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
            Val::Str(s) => s.trim().replace([',', '_'], "").parse().ok(),
            _ => None,
        }
    }
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Val::Str(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_list(&self) -> Option<&[Val]> {
        match self {
            Val::List(l) => Some(l),
            _ => None,
        }
    }
    /// A field of an object (Null when absent or not an object).
    pub fn field(&self, k: &str) -> &Val {
        match self {
            Val::Obj(o) => o.iter().find(|(x, _)| x == k).map(|(_, v)| v).unwrap_or(&Val::Null),
            _ => &Val::Null,
        }
    }
    /// What JavaScript's String(x) would say (lists joined by commas; null is empty).
    pub fn text(&self) -> String {
        match self {
            Val::Null => String::new(),
            Val::Bool(b) => b.to_string(),
            Val::Num(n) => num_text(*n),
            Val::Str(s) => s.clone(),
            Val::List(l) => l.iter().map(Val::text).collect::<Vec<_>>().join(","),
            Val::Obj(_) => json(self),
            Val::Node(n) => n.name.clone(),
            Val::Func(_) => "function".into(),
        }
    }
    fn type_name(&self) -> &'static str {
        match self {
            Val::Null => "null",
            Val::Bool(_) => "a true/false",
            Val::Num(_) => "a number",
            Val::Str(_) => "a text",
            Val::List(_) => "a list",
            Val::Obj(_) => "an object",
            Val::Node(_) => "a component",
            Val::Func(_) => "a function",
        }
    }
}

/// A number as JavaScript prints it (no ".0" on whole numbers).
pub fn num_text(n: f64) -> String {
    if n.is_nan() {
        "NaN".into()
    } else if n.is_infinite() {
        if n > 0.0 { "Infinity".into() } else { "-Infinity".into() }
    } else if n.fract() == 0.0 && n.abs() < 1e21 {
        format!("{}", n as i128)
    } else {
        format!("{n}")
    }
}

/// `fmt(x, d)`: grouped thousands; `d` fixed decimals, or up to 3 when not given ("1,234.5").
pub fn fmt_num(n: f64, digits: Option<usize>) -> String {
    if !n.is_finite() {
        return num_text(n);
    }
    let d = digits.unwrap_or(3).min(12);
    let s = format!("{:.*}", d, n.abs());
    let (int, frac) = s.split_once('.').unwrap_or((&s, ""));
    let mut g = String::new();
    for (i, c) in int.chars().enumerate() {
        if i > 0 && (int.len() - i) % 3 == 0 {
            g.push(',');
        }
        g.push(c);
    }
    let frac = if digits.is_none() { frac.trim_end_matches('0') } else { frac };
    let neg = n < 0.0 && (int.chars().any(|c| c != '0') || frac.chars().any(|c| c != '0'));
    format!("{}{g}{}{frac}", if neg { "-" } else { "" }, if frac.is_empty() { "" } else { "." })
}

fn json(v: &Val) -> String {
    match v {
        Val::Str(s) => format!("{s:?}"),
        Val::List(l) => format!("[{}]", l.iter().map(json).collect::<Vec<_>>().join(",")),
        Val::Obj(o) => format!("{{{}}}", o.iter().map(|(k, v)| format!("{k:?}:{}", json(v))).collect::<Vec<_>>().join(",")),
        Val::Null => "null".into(),
        v => v.text(),
    }
}

impl Node {
    /// The argument named `name`, else the `i`th positional one.
    pub fn get(&self, i: usize, name: &str) -> &Val {
        self.named.iter().find(|(k, _)| k == name).map(|(_, v)| v).or_else(|| self.args.get(i)).unwrap_or(&Val::Null)
    }
    pub fn str(&self, i: usize, name: &str) -> String {
        self.get(i, name).text()
    }
    pub fn num(&self, i: usize, name: &str) -> Option<f64> {
        self.get(i, name).as_num()
    }
    /// The `$state` the `i`th argument was written as.
    pub fn bind(&self, i: usize) -> Option<&str> {
        self.binds.get(i).and_then(|b| b.as_deref())
    }
    /// A container's children, flattened (null and false dropped).
    pub fn kids(&self) -> Vec<Val> {
        let Some((i, name)) = child_slot(&self.name) else { return vec![] };
        // Card([…]) with no title: the list is the children.
        let v = match self.get(i, name) {
            Val::Null if i == 1 && matches!(self.args.first(), Some(Val::List(_) | Val::Node(_))) => &self.args[0],
            v => v,
        };
        flatten(v)
    }
    /// A container's title (empty when its first argument is the children).
    pub fn title(&self) -> String {
        match self.get(0, "title") {
            Val::List(_) | Val::Node(_) => String::new(),
            v => v.text(),
        }
    }
    pub fn tone(&self, i: usize) -> String {
        let t = self.str(i, "tone");
        if matches!(t.as_str(), "good" | "warn" | "bad" | "info" | "neutral") { t } else { "neutral".into() }
    }
}

pub fn error_node(msg: impl Into<String>) -> Val {
    Val::Node(Arc::new(Node { name: "Error".into(), args: vec![Val::Str(msg.into())], named: vec![], binds: vec![] }))
}

/// Which argument holds a container's children.
pub fn child_slot(name: &str) -> Option<(usize, &'static str)> {
    match name {
        "Card" | "Tabs" | "Details" => Some((1, "children")),
        "Row" | "Stack" | "Grid" | "Form" => Some((0, "children")),
        _ => None,
    }
}

/// Inputs: bound to a `$state` (their first argument).
pub fn is_input(name: &str) -> bool {
    matches!(name, "Slider" | "Toggle" | "Switch" | "Choice" | "Radio" | "Select" | "TextInput" | "Input" | "TextArea" | "Checklist" | "Stepper")
}

pub fn is_action(name: &str) -> bool {
    matches!(name, "Send" | "Set" | "OpenUrl" | "Copy")
}

/// Children as drawn: lists flattened, null and true/false dropped.
pub fn flatten(v: &Val) -> Vec<Val> {
    match v {
        Val::List(l) => l.iter().flat_map(flatten).collect(),
        Val::Null | Val::Bool(_) => vec![],
        v => vec![v.clone()],
    }
}

/// The layout of what is drawn, without its values: containers and their children, inputs and the
/// state they bind. The web part rebuilds the DOM only when this changes; values update in place.
pub fn shape(roots: &[Val]) -> String {
    fn go(v: &Val, out: &mut String) {
        match v {
            Val::Node(n) if child_slot(&n.name).is_some() => {
                out.push_str(&n.name);
                out.push('(');
                for k in n.kids() {
                    go(&k, out);
                    out.push(',');
                }
                out.push(')');
            }
            Val::Node(n) if is_input(&n.name) => {
                out.push_str(&n.name);
                out.push(':');
                out.push_str(n.bind(0).unwrap_or(""));
            }
            Val::Node(n) => out.push_str(&n.name),
            _ => out.push('s'),
        }
    }
    let mut s = String::new();
    for r in roots {
        go(r, &mut s);
        s.push(';');
    }
    s
}

/// The value at a path: a root's index, then a child's index in each container.
pub fn at(roots: &[Val], path: &[usize]) -> Option<Val> {
    let mut v = roots.get(*path.first()?)?.clone();
    for &i in &path[1..] {
        let Val::Node(n) = &v else { return None };
        v = n.kids().into_iter().nth(i)?;
    }
    Some(v)
}

// ───────────────────────────── tokens ─────────────────────────────

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Num(f64),
    Str(String),
    /// Text pieces, and the source of each `${…}` between them.
    Tpl(Vec<String>, Vec<String>),
    Ident(String),
    State(String),
    P(&'static str),
    Sep,
}

#[derive(Clone, Debug)]
struct Token {
    t: Tok,
    line: u32,
}

const PUNCT: &[&str] = &[
    "===", "!==", "...", "==", "!=", "<=", ">=", "&&", "||", "??", "=>", "**", "(", ")", "[", "]", "{", "}", ",", ":", ".", "?", "<", ">", "+", "-", "*", "/", "%", "!", "=",
];
/// A newline right after one of these continues the statement.
const CONT_AFTER: &[&str] = &["+", "-", "*", "/", "%", "**", "==", "!=", "===", "!==", "<", "<=", ">", ">=", "&&", "||", "??", "?", ":", ".", "=", "=>", ","];
/// A line starting with one of these continues the one before (none can start an expression).
const CONT_BEFORE: &[&str] = &["+", "*", "/", "%", "**", "==", "!=", "===", "!==", "<", "<=", ">", ">=", "&&", "||", "??", "?", ":", ".", "=>"];

fn describe(t: &Tok) -> String {
    match t {
        Tok::Num(n) => num_text(*n),
        Tok::Str(s) => format!("{s:?}"),
        Tok::Tpl(..) => "a `template`".into(),
        Tok::Ident(s) => s.clone(),
        Tok::State(s) => format!("${s}"),
        Tok::P(p) => format!("'{p}'"),
        Tok::Sep => "the end of the line".into(),
    }
}

/// Tokens, and the error that stopped the tokenizer (the tokens before it are kept).
fn lex(src: &str) -> (Vec<Token>, Option<String>) {
    let cs: Vec<char> = src.chars().collect();
    let (mut i, mut line, mut depth) = (0usize, 1u32, 0i32);
    let mut out: Vec<Token> = vec![];
    let sep = |out: &mut Vec<Token>, line: u32| {
        let cont = match out.last() {
            None | Some(Token { t: Tok::Sep, .. }) => true,
            Some(Token { t: Tok::P(p), .. }) => CONT_AFTER.contains(p),
            _ => false,
        };
        if !cont {
            out.push(Token { t: Tok::Sep, line });
        }
    };
    while i < cs.len() {
        let c = cs[i];
        let next = cs.get(i + 1).copied();
        if c == '\n' {
            if depth <= 0 {
                sep(&mut out, line);
            }
            line += 1;
            i += 1;
            continue;
        }
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        if c == '/' && next == Some('/') {
            while i < cs.len() && cs[i] != '\n' {
                i += 1;
            }
            continue;
        }
        if c == ';' {
            if !matches!(out.last(), None | Some(Token { t: Tok::Sep, .. })) {
                out.push(Token { t: Tok::Sep, line });
            }
            i += 1;
            continue;
        }
        let at_line = line;
        let tok = if c.is_ascii_digit() {
            let start = i;
            let digits = |i: &mut usize| {
                while *i < cs.len() && (cs[*i].is_ascii_digit() || cs[*i] == '_') {
                    *i += 1;
                }
            };
            digits(&mut i);
            if i + 1 < cs.len() && cs[i] == '.' && cs[i + 1].is_ascii_digit() {
                i += 1;
                digits(&mut i);
            }
            if i < cs.len() && (cs[i] == 'e' || cs[i] == 'E') {
                let mut j = i + 1;
                if j < cs.len() && (cs[j] == '+' || cs[j] == '-') {
                    j += 1;
                }
                if j < cs.len() && cs[j].is_ascii_digit() {
                    i = j;
                    digits(&mut i);
                }
            }
            let s: String = cs[start..i].iter().filter(|c| **c != '_').collect();
            match s.parse::<f64>() {
                Ok(n) => Tok::Num(n),
                Err(_) => return (out, Some(format!("line {line}: {s} isn't a number"))),
            }
        } else if c == '"' || c == '\'' {
            i += 1;
            let mut s = String::new();
            loop {
                match cs.get(i) {
                    None | Some('\n') => return (out, Some(format!("line {at_line}: a text is missing its closing {c}"))),
                    Some(&q) if q == c => {
                        i += 1;
                        break;
                    }
                    Some('\\') => {
                        i += 1;
                        s.push(escape(&cs, &mut i));
                    }
                    Some(&ch) => {
                        s.push(ch);
                        i += 1;
                    }
                }
            }
            Tok::Str(s)
        } else if c == '`' {
            i += 1;
            let (mut quasis, mut parts) = (vec![String::new()], vec![]);
            loop {
                match cs.get(i) {
                    None => return (out, Some(format!("line {at_line}: a `template` is missing its closing `"))),
                    Some('`') => {
                        i += 1;
                        break;
                    }
                    Some('\\') => {
                        i += 1;
                        let ch = escape(&cs, &mut i);
                        quasis.last_mut().unwrap().push(ch);
                    }
                    Some('$') if cs.get(i + 1) == Some(&'{') => {
                        i += 2;
                        let start = i;
                        let mut d = 1;
                        let mut quote: Option<char> = None;
                        while i < cs.len() {
                            let ch = cs[i];
                            match quote {
                                Some(q) => {
                                    if ch == '\\' {
                                        i += 1;
                                    } else if ch == q {
                                        quote = None;
                                    }
                                }
                                None => match ch {
                                    '"' | '\'' | '`' => quote = Some(ch),
                                    '{' => d += 1,
                                    '}' => {
                                        d -= 1;
                                        if d == 0 {
                                            break;
                                        }
                                    }
                                    _ => {}
                                },
                            }
                            if ch == '\n' {
                                line += 1;
                            }
                            i += 1;
                        }
                        if i >= cs.len() {
                            return (out, Some(format!("line {at_line}: a ${{…}} in a template isn't closed")));
                        }
                        parts.push(cs[start..i].iter().collect());
                        quasis.push(String::new());
                        i += 1;
                    }
                    Some(&ch) => {
                        if ch == '\n' {
                            line += 1;
                        }
                        quasis.last_mut().unwrap().push(ch);
                        i += 1;
                    }
                }
            }
            Tok::Tpl(quasis, parts)
        } else if c == '$' && next.is_some_and(|n| n.is_ascii_alphabetic() || n == '_') {
            i += 1;
            let start = i;
            while i < cs.len() && (cs[i].is_ascii_alphanumeric() || cs[i] == '_') {
                i += 1;
            }
            Tok::State(cs[start..i].iter().collect())
        } else if c.is_alphabetic() || c == '_' {
            let start = i;
            while i < cs.len() && (cs[i].is_alphanumeric() || cs[i] == '_') {
                i += 1;
            }
            Tok::Ident(cs[start..i].iter().collect())
        } else {
            let Some(p) = PUNCT.iter().find(|p| p.chars().enumerate().all(|(k, pc)| cs.get(i + k) == Some(&pc))) else {
                return (out, Some(format!("line {line}: unexpected '{c}'")));
            };
            i += p.chars().count();
            match *p {
                "(" | "[" | "{" => depth += 1,
                ")" | "]" | "}" => depth -= 1,
                _ => {}
            }
            if CONT_BEFORE.contains(p) && matches!(out.last(), Some(Token { t: Tok::Sep, .. })) {
                out.pop();
            }
            Tok::P(p)
        };
        out.push(Token { t: tok, line: at_line });
    }
    (out, None)
}

fn escape(cs: &[char], i: &mut usize) -> char {
    let Some(&c) = cs.get(*i) else { return '\\' };
    *i += 1;
    match c {
        'n' => '\n',
        't' => '\t',
        'r' => '\r',
        '0' => '\0',
        'u' => {
            let hex: String = if cs.get(*i) == Some(&'{') {
                let start = *i + 1;
                let end = (start..cs.len()).find(|k| cs[*k] == '}').unwrap_or(cs.len());
                *i = (end + 1).min(cs.len());
                cs[start..end].iter().collect()
            } else {
                let end = (*i + 4).min(cs.len());
                let h: String = cs[*i..end].iter().collect();
                *i = end;
                h
            };
            u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32).unwrap_or('\u{fffd}')
        }
        c => c,
    }
}

// ───────────────────────────── syntax ─────────────────────────────

#[derive(Debug, PartialEq)]
pub enum Expr {
    Lit(Val),
    Tpl(Vec<String>, Vec<Expr>),
    State(String),
    Ident(String),
    List(Vec<Expr>),
    Obj(Vec<(Option<String>, Expr)>),
    /// `...x` in a list (or, with no key, in an object).
    Spread(Box<Expr>),
    Unary(&'static str, Box<Expr>),
    Bin(&'static str, Box<Expr>, Box<Expr>),
    Cond(Box<Expr>, Box<Expr>, Box<Expr>),
    Member(Box<Expr>, String),
    Index(Box<Expr>, Box<Expr>),
    Call(String, Vec<Expr>, Vec<(String, Expr)>),
    Method(Box<Expr>, String, Vec<Expr>, Vec<(String, Expr)>),
    Arrow(Vec<String>, Arc<Expr>),
}

#[derive(Debug)]
pub enum Stmt {
    State(String, Expr),
    Named(String, Expr),
    Root(Expr),
    /// A line that didn't parse, and why.
    Bad(String),
}

struct Parser {
    toks: Vec<Tok>,
    i: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.i)
    }
    fn peek_at(&self, k: usize) -> Option<&Tok> {
        self.toks.get(self.i + k)
    }
    fn is_p(&self, p: &str) -> bool {
        matches!(self.peek(), Some(Tok::P(q)) if *q == p)
    }
    fn eat_p(&mut self, p: &str) -> bool {
        let y = self.is_p(p);
        if y {
            self.i += 1;
        }
        y
    }
    fn found(&self) -> String {
        self.peek().map(describe).unwrap_or_else(|| "the end of the line".into())
    }
    fn expect_p(&mut self, p: &str) -> R<()> {
        if self.eat_p(p) { Ok(()) } else { Err(format!("expected '{p}' but found {}", self.found())) }
    }

    fn expr(&mut self) -> R<Expr> {
        if let Some(params) = self.arrow_params() {
            let body = self.expr()?;
            return Ok(Expr::Arrow(params, Arc::new(body)));
        }
        let test = self.binary(0)?;
        if self.eat_p("?") {
            let a = self.expr()?;
            self.expect_p(":")?;
            let b = self.expr()?;
            return Ok(Expr::Cond(Box::new(test), Box::new(a), Box::new(b)));
        }
        Ok(test)
    }

    /// `x =>` or `(a, b) =>`: the parameters (and the tokens are consumed through the arrow).
    fn arrow_params(&mut self) -> Option<Vec<String>> {
        if let (Some(Tok::Ident(n)), Some(Tok::P("=>"))) = (self.peek(), self.peek_at(1)) {
            let n = n.clone();
            self.i += 2;
            return Some(vec![n]);
        }
        if !self.is_p("(") {
            return None;
        }
        let mut k = 1;
        let mut params = vec![];
        loop {
            match self.peek_at(k) {
                Some(Tok::Ident(n)) => {
                    params.push(n.clone());
                    k += 1;
                    match self.peek_at(k) {
                        Some(Tok::P(",")) => k += 1,
                        Some(Tok::P(")")) => {}
                        _ => return None,
                    }
                }
                Some(Tok::P(")")) => break,
                _ => return None,
            }
        }
        if self.peek_at(k + 1) != Some(&Tok::P("=>")) {
            return None;
        }
        self.i += k + 2;
        Some(params)
    }

    fn binary(&mut self, min: u8) -> R<Expr> {
        fn prec(op: &str) -> Option<(u8, bool)> {
            Some(match op {
                "??" => (1, false),
                "||" => (2, false),
                "&&" => (3, false),
                "==" | "!=" | "===" | "!==" => (4, false),
                "<" | "<=" | ">" | ">=" => (5, false),
                "+" | "-" => (6, false),
                "*" | "/" | "%" => (7, false),
                "**" => (8, true),
                _ => return None,
            })
        }
        let mut left = self.unary()?;
        loop {
            let Some(Tok::P(op)) = self.peek() else { break };
            let op: &'static str = op;
            let Some((p, right)) = prec(op) else { break };
            if p < min {
                break;
            }
            self.i += 1;
            let rhs = self.binary(if right { p } else { p + 1 })?;
            left = Expr::Bin(op, Box::new(left), Box::new(rhs));
        }
        Ok(left)
    }

    fn unary(&mut self) -> R<Expr> {
        for op in ["!", "-", "+"] {
            if self.eat_p(op) {
                return Ok(Expr::Unary(op, Box::new(self.unary()?)));
            }
        }
        self.postfix()
    }

    fn postfix(&mut self) -> R<Expr> {
        let mut e = self.primary()?;
        loop {
            let optional = self.is_p("?") && self.peek_at(1) == Some(&Tok::P("."));
            if self.is_p(".") || optional {
                self.i += if optional { 2 } else { 1 };
                let name = match self.peek() {
                    Some(Tok::Ident(n)) => n.clone(),
                    _ => return Err(format!("expected a name after '.' but found {}", self.found())),
                };
                self.i += 1;
                if self.is_p("(") {
                    let (args, named) = self.args()?;
                    e = Expr::Method(Box::new(e), name, args, named);
                } else {
                    e = Expr::Member(Box::new(e), name);
                }
            } else if self.eat_p("[") {
                let idx = self.expr()?;
                self.expect_p("]")?;
                e = Expr::Index(Box::new(e), Box::new(idx));
            } else {
                break;
            }
        }
        Ok(e)
    }

    fn args(&mut self) -> R<(Vec<Expr>, Vec<(String, Expr)>)> {
        self.expect_p("(")?;
        let (mut args, mut named) = (vec![], vec![]);
        while !self.is_p(")") {
            if let (Some(Tok::Ident(n)), Some(Tok::P(":"))) = (self.peek(), self.peek_at(1)) {
                let n = n.clone();
                self.i += 2;
                named.push((n, self.expr()?));
            } else if self.eat_p("...") {
                args.push(Expr::Spread(Box::new(self.expr()?)));
            } else {
                args.push(self.expr()?);
            }
            if !self.eat_p(",") {
                break;
            }
        }
        self.expect_p(")")?;
        Ok((args, named))
    }

    fn primary(&mut self) -> R<Expr> {
        let Some(t) = self.peek().cloned() else { return Err("the line ends too soon".into()) };
        self.i += 1;
        Ok(match t {
            Tok::Num(n) => Expr::Lit(Val::Num(n)),
            Tok::Str(s) => Expr::Lit(Val::Str(s)),
            Tok::Tpl(quasis, parts) => {
                let mut es = vec![];
                for p in parts {
                    es.push(parse_expr(&p)?);
                }
                Expr::Tpl(quasis, es)
            }
            Tok::State(n) => Expr::State(n),
            Tok::Ident(n) => match n.as_str() {
                "true" => Expr::Lit(Val::Bool(true)),
                "false" => Expr::Lit(Val::Bool(false)),
                "null" | "undefined" => Expr::Lit(Val::Null),
                _ if self.is_p("(") => {
                    let (args, named) = self.args()?;
                    Expr::Call(n, args, named)
                }
                _ => Expr::Ident(n),
            },
            Tok::P("(") => {
                let e = self.expr()?;
                self.expect_p(")")?;
                e
            }
            Tok::P("[") => {
                let mut items = vec![];
                while !self.is_p("]") {
                    if self.eat_p("...") {
                        items.push(Expr::Spread(Box::new(self.expr()?)));
                    } else {
                        items.push(self.expr()?);
                    }
                    if !self.eat_p(",") {
                        break;
                    }
                }
                self.expect_p("]")?;
                Expr::List(items)
            }
            Tok::P("{") => {
                let mut entries = vec![];
                while !self.is_p("}") {
                    if self.eat_p("...") {
                        entries.push((None, self.expr()?));
                    } else {
                        let key = match self.peek().cloned() {
                            Some(Tok::Ident(k)) | Some(Tok::Str(k)) => k,
                            Some(Tok::Num(n)) => num_text(n),
                            _ => return Err(format!("expected a key in the object but found {}", self.found())),
                        };
                        self.i += 1;
                        if self.eat_p(":") {
                            entries.push((Some(key), self.expr()?));
                        } else {
                            entries.push((Some(key.clone()), Expr::Ident(key)));
                        }
                    }
                    if !self.eat_p(",") {
                        break;
                    }
                }
                self.expect_p("}")?;
                Expr::Obj(entries)
            }
            t => return Err(format!("unexpected {}", describe(&t))),
        })
    }
}

fn parse_expr(src: &str) -> R<Expr> {
    let (toks, err) = lex(src);
    if let Some(e) = err {
        return Err(e);
    }
    let toks: Vec<Tok> = toks.into_iter().map(|t| t.t).filter(|t| *t != Tok::Sep).collect();
    let mut p = Parser { toks, i: 0 };
    let e = p.expr()?;
    if p.i < p.toks.len() {
        return Err(format!("unexpected {} in ${{…}}", p.found()));
    }
    Ok(e)
}

fn statement(toks: Vec<Tok>) -> R<Stmt> {
    let (decl, skip) = match (toks.first(), toks.get(1)) {
        (Some(Tok::State(n)), Some(Tok::P("="))) => (Some((true, n.clone())), 2),
        (Some(Tok::Ident(n)), Some(Tok::P("="))) => (Some((false, n.clone())), 2),
        _ => (None, 0),
    };
    let mut p = Parser { toks, i: skip };
    let e = p.expr()?;
    if p.i < p.toks.len() {
        return Err(format!("unexpected {} after the expression", p.found()));
    }
    Ok(match decl {
        Some((true, n)) => Stmt::State(n, e),
        Some((false, n)) => Stmt::Named(n, e),
        None => Stmt::Root(e),
    })
}

/// A parsed block.
#[derive(Debug)]
pub struct Program {
    /// Each statement and the line it starts on.
    pub stmts: Vec<(u32, Stmt)>,
    named: BTreeMap<String, Arc<Expr>>,
}

impl Program {
    pub fn parse(src: &str) -> Program {
        let (toks, err) = lex(src);
        let mut stmts = vec![];
        let mut cur: Vec<Token> = vec![];
        let flush = |cur: &mut Vec<Token>, stmts: &mut Vec<(u32, Stmt)>| {
            if cur.is_empty() {
                return;
            }
            let line = cur[0].line;
            let toks = std::mem::take(cur).into_iter().map(|t| t.t).collect();
            stmts.push((line, statement(toks).unwrap_or_else(|e| Stmt::Bad(format!("line {line}: {e}")))));
        };
        for t in toks {
            if t.t == Tok::Sep {
                flush(&mut cur, &mut stmts);
            } else {
                cur.push(t);
            }
        }
        if let Some(e) = err {
            // The statement the tokenizer stopped in is the broken one.
            cur.clear();
            stmts.push((0, Stmt::Bad(e)));
        }
        flush(&mut cur, &mut stmts);
        let mut named = BTreeMap::new();
        for (_, s) in &mut stmts {
            if let Stmt::Named(n, e) = s {
                let e = std::mem::replace(e, Expr::Lit(Val::Null));
                named.insert(n.clone(), Arc::new(e));
            }
        }
        Program { stmts, named }
    }

    /// Every `$state`'s starting value, in order, and the errors of those that failed.
    pub fn initial_state(&self) -> (State, Vec<String>) {
        let mut state = State::new();
        let mut errs = vec![];
        for (line, s) in &self.stmts {
            if let Stmt::State(n, e) = s {
                let v = Ev::new(self, &state).eval(e);
                match v {
                    Ok(v) => {
                        state.insert(n.clone(), v);
                    }
                    Err(e) => {
                        errs.push(format!("line {line}: ${n}: {e}"));
                        state.insert(n.clone(), Val::Null);
                    }
                }
            }
        }
        (state, errs)
    }

    /// What to draw, for this state: each bare expression (lists flattened), or `root`.
    pub fn roots(&self, state: &State) -> Vec<Val> {
        let mut ev = Ev::new(self, state);
        let mut out = vec![];
        let mut any = false;
        for (line, s) in &self.stmts {
            match s {
                Stmt::Root(e) => {
                    any = true;
                    match ev.eval(e) {
                        Ok(v) => out.extend(flatten(&v)),
                        Err(e) => out.push(error_node(format!("line {line}: {e}"))),
                    }
                }
                Stmt::Bad(msg) => out.push(error_node(msg.clone())),
                _ => {}
            }
        }
        if !any && self.named.contains_key("root") {
            match ev.named("root") {
                Ok(v) => out.extend(flatten(&v)),
                Err(e) => out.push(error_node(e)),
            }
        }
        out
    }
}

// ───────────────────────────── evaluation ─────────────────────────────

struct Ev<'a> {
    prog: &'a Program,
    state: &'a State,
    /// Named values evaluated so far (None: being evaluated, to catch cycles).
    memo: BTreeMap<String, Option<Val>>,
    locals: Vec<(String, Val)>,
    depth: u32,
    budget: u64,
}

const MAX_DEPTH: u32 = 120;
const MAX_LIST: usize = 10_000;

impl<'a> Ev<'a> {
    fn new(prog: &'a Program, state: &'a State) -> Ev<'a> {
        Ev { prog, state, memo: BTreeMap::new(), locals: vec![], depth: 0, budget: 2_000_000 }
    }

    fn named(&mut self, name: &str) -> R<Val> {
        match self.memo.get(name) {
            Some(Some(v)) => return Ok(v.clone()),
            Some(None) => return Err(format!("{name} is defined in terms of itself")),
            None => {}
        }
        let e = self.prog.named.get(name).cloned().ok_or_else(|| format!("{name} isn't defined"))?;
        self.memo.insert(name.to_string(), None);
        let saved = std::mem::take(&mut self.locals);
        let v = self.eval(&e);
        self.locals = saved;
        match v {
            Ok(v) => {
                self.memo.insert(name.to_string(), Some(v.clone()));
                Ok(v)
            }
            Err(e) => {
                self.memo.remove(name);
                Err(e)
            }
        }
    }

    fn lookup(&mut self, name: &str) -> R<Val> {
        if let Some((_, v)) = self.locals.iter().rev().find(|(k, _)| k == name) {
            return Ok(v.clone());
        }
        if self.prog.named.contains_key(name) {
            return self.named(name);
        }
        Err(format!("{name} isn't defined"))
    }

    fn bound(&self, name: &str) -> bool {
        self.locals.iter().any(|(k, _)| k == name) || self.prog.named.contains_key(name)
    }

    fn call_func(&mut self, f: &Val, args: Vec<Val>) -> R<Val> {
        let Val::Func(f) = f else { return Err(format!("expected a function (like x => x * 2), got {}", f.type_name())) };
        if self.depth >= MAX_DEPTH {
            return Err("too many nested calls".into());
        }
        let mut locals = f.env.clone();
        for (i, p) in f.params.iter().enumerate() {
            locals.push((p.clone(), args.get(i).cloned().unwrap_or(Val::Null)));
        }
        let saved = std::mem::replace(&mut self.locals, locals);
        self.depth += 1;
        let v = self.eval(&f.body);
        self.depth -= 1;
        self.locals = saved;
        v
    }

    fn eval_all(&mut self, es: &[Expr]) -> R<Vec<Val>> {
        let mut out = vec![];
        for e in es {
            match e {
                Expr::Spread(x) => match self.eval(x)? {
                    Val::List(l) => out.extend(l),
                    Val::Null => {}
                    v => return Err(format!("...spread needs a list, got {}", v.type_name())),
                },
                e => out.push(self.eval(e)?),
            }
        }
        Ok(out)
    }

    fn eval(&mut self, e: &Expr) -> R<Val> {
        if self.budget == 0 {
            return Err("this block does too much work to draw".into());
        }
        self.budget -= 1;
        Ok(match e {
            Expr::Lit(v) => v.clone(),
            Expr::Tpl(quasis, parts) => {
                let mut s = quasis[0].clone();
                for (i, p) in parts.iter().enumerate() {
                    s += &self.eval(p)?.text();
                    s += quasis.get(i + 1).map(String::as_str).unwrap_or("");
                }
                Val::Str(s)
            }
            Expr::State(n) => self.state.get(n).cloned().ok_or_else(|| format!("${n} isn't declared (add a line `${n} = …`)"))?,
            Expr::Ident(n) => match n.as_str() {
                "Infinity" => Val::Num(f64::INFINITY),
                "NaN" => Val::Num(f64::NAN),
                _ => self.lookup(n)?,
            },
            Expr::List(items) => Val::List(self.eval_all(items)?),
            Expr::Obj(entries) => {
                let mut o: Vec<(String, Val)> = vec![];
                for (k, e) in entries {
                    match k {
                        Some(k) => {
                            let v = self.eval(e)?;
                            o.retain(|(x, _)| x != k);
                            o.push((k.clone(), v));
                        }
                        None => match self.eval(e)? {
                            Val::Obj(more) => {
                                for (k, v) in more {
                                    o.retain(|(x, _)| *x != k);
                                    o.push((k, v));
                                }
                            }
                            Val::Null => {}
                            v => return Err(format!("...spread in an object needs an object, got {}", v.type_name())),
                        },
                    }
                }
                Val::Obj(o)
            }
            Expr::Spread(_) => return Err("...spread only works inside [ ] or { }".into()),
            Expr::Unary(op, x) => {
                let v = self.eval(x)?;
                match *op {
                    "!" => Val::Bool(!v.truthy()),
                    _ => {
                        let n = num(&v, op)?;
                        Val::Num(if *op == "-" { -n } else { n })
                    }
                }
            }
            Expr::Bin(op, a, b) => {
                let a = self.eval(a)?;
                match *op {
                    "&&" => return if a.truthy() { self.eval(b) } else { Ok(a) },
                    "||" => return if a.truthy() { Ok(a) } else { self.eval(b) },
                    "??" => return if matches!(a, Val::Null) { self.eval(b) } else { Ok(a) },
                    _ => {}
                }
                let b = self.eval(b)?;
                binop(op, &a, &b)?
            }
            Expr::Cond(t, a, b) => {
                if self.eval(t)?.truthy() {
                    self.eval(a)?
                } else {
                    self.eval(b)?
                }
            }
            Expr::Member(o, name) => {
                if let Expr::Ident(g) = o.as_ref() {
                    if !self.bound(g) {
                        if let Some(v) = global_const(g, name) {
                            return Ok(v);
                        }
                    }
                }
                let v = self.eval(o)?;
                member(&v, name)
            }
            Expr::Index(o, i) => {
                let v = self.eval(o)?;
                let i = self.eval(i)?;
                match (&v, &i) {
                    (Val::List(l), Val::Num(n)) => idx(l.len(), *n).and_then(|k| l.get(k).cloned()).unwrap_or(Val::Null),
                    (Val::Str(s), Val::Num(n)) => idx(s.chars().count(), *n).and_then(|k| s.chars().nth(k)).map(|c| Val::Str(c.to_string())).unwrap_or(Val::Null),
                    (_, Val::Str(k)) => member(&v, k),
                    (_, Val::Num(n)) => member(&v, &num_text(*n)),
                    _ => Val::Null,
                }
            }
            Expr::Arrow(params, body) => Val::Func(Arc::new(Func { params: params.clone(), body: body.clone(), env: self.locals.clone() })),
            Expr::Call(name, args, named) => return self.call(name, args, named),
            Expr::Method(o, name, args, _) => {
                if let Expr::Ident(g) = o.as_ref() {
                    if !self.bound(g) && matches!(g.as_str(), "Math" | "Object" | "JSON" | "Number" | "Array") {
                        let args = self.eval_all(args)?;
                        return global_method(g, name, &args);
                    }
                }
                let v = self.eval(o)?;
                let args = self.eval_all(args)?;
                return self.method(&v, name, args);
            }
        })
    }

    fn call(&mut self, name: &str, args: &[Expr], named: &[(String, Expr)]) -> R<Val> {
        // A function the block defined (or an arrow parameter holding one).
        if self.bound(name) {
            let f = self.lookup(name)?;
            let args = self.eval_all(args)?;
            return self.call_func(&f, args);
        }
        if let Some(r) = self.builtin(name, args)? {
            return Ok(r);
        }
        if !name.starts_with(|c: char| c.is_uppercase()) {
            return Err(format!("there's no function {name}()"));
        }
        if name == "Load" {
            return Ok(error_node("Load(…) reads files from the machine, which Agentum's ui blocks can't do yet."));
        }
        let built = (|| -> R<Node> {
            let binds = args.iter().map(|a| if let Expr::State(n) = a { Some(n.clone()) } else { None }).collect();
            let args = self.eval_all(args)?;
            let mut nv = vec![];
            for (k, e) in named {
                nv.push((k.clone(), self.eval(e)?));
            }
            Ok(Node { name: name.to_string(), args, named: nv, binds })
        })();
        match built {
            Ok(n) => Ok(Val::Node(Arc::new(n))),
            Err(e) if is_action(name) => Err(e),
            // A component whose arguments failed shows why, in its place.
            Err(e) => Ok(error_node(format!("{name}: {e}"))),
        }
    }

    /// The pure functions: round, fmt, sum, min, max, abs, entries…
    fn builtin(&mut self, name: &str, args: &[Expr]) -> R<Option<Val>> {
        const NAMES: &[&str] = &[
            "round", "fmt", "sum", "min", "max", "abs", "floor", "ceil", "sqrt", "pow", "avg", "mean", "entries", "keys", "values", "len", "count", "range", "String", "Number", "Boolean", "pct", "clamp", "upper", "lower", "join", "first", "last", "json",
        ];
        if !NAMES.contains(&name) {
            return Ok(None);
        }
        let a = self.eval_all(args)?;
        let arg = |i: usize| a.get(i).cloned().unwrap_or(Val::Null);
        let n = |i: usize| num(&arg(i), name);
        let nums = |a: &[Val]| -> R<Vec<f64>> {
            let list: Vec<Val> = if a.len() == 1 { a[0].as_list().map(<[Val]>::to_vec).unwrap_or_else(|| a.to_vec()) } else { a.to_vec() };
            list.iter().filter(|v| !matches!(v, Val::Null)).map(|v| num(v, name)).collect()
        };
        Ok(Some(match name {
            "round" => {
                let p = 10f64.powi(arg(1).as_num().unwrap_or(0.0).clamp(0.0, 12.0) as i32);
                Val::Num((n(0)? * p + 0.5).floor() / p)
            }
            "fmt" => {
                let v = arg(0);
                let d = arg(1).as_num().map(|d| d.clamp(0.0, 12.0) as usize);
                match v.as_num() {
                    Some(x) => Val::Str(fmt_num(x, d)),
                    None => Val::Str(v.text()),
                }
            }
            "sum" => Val::Num(nums(&a)?.iter().sum()),
            "avg" | "mean" => {
                let v = nums(&a)?;
                if v.is_empty() { Val::Null } else { Val::Num(v.iter().sum::<f64>() / v.len() as f64) }
            }
            "min" => nums(&a)?.into_iter().reduce(f64::min).map(Val::Num).unwrap_or(Val::Null),
            "max" => nums(&a)?.into_iter().reduce(f64::max).map(Val::Num).unwrap_or(Val::Null),
            "abs" => Val::Num(n(0)?.abs()),
            "floor" => Val::Num(n(0)?.floor()),
            "ceil" => Val::Num(n(0)?.ceil()),
            "sqrt" => Val::Num(n(0)?.sqrt()),
            "pow" => Val::Num(n(0)?.powf(n(1)?)),
            "clamp" => Val::Num(n(0)?.max(n(1)?).min(n(2)?)),
            "pct" => {
                let (x, of) = (n(0)?, n(1)?);
                Val::Num(if of == 0.0 { 0.0 } else { x / of * 100.0 })
            }
            "entries" => match arg(0) {
                Val::Obj(o) => Val::List(o.into_iter().map(|(k, v)| Val::List(vec![Val::Str(k), v])).collect()),
                Val::List(l) => Val::List(l.into_iter().enumerate().map(|(i, v)| Val::List(vec![Val::Num(i as f64), v])).collect()),
                _ => Val::List(vec![]),
            },
            "keys" => match arg(0) {
                Val::Obj(o) => Val::List(o.into_iter().map(|(k, _)| Val::Str(k)).collect()),
                _ => Val::List(vec![]),
            },
            "values" => match arg(0) {
                Val::Obj(o) => Val::List(o.into_iter().map(|(_, v)| v).collect()),
                v => v,
            },
            "len" | "count" => member(&arg(0), "length"),
            "first" => arg(0).as_list().and_then(|l| l.first().cloned()).unwrap_or(Val::Null),
            "last" => arg(0).as_list().and_then(|l| l.last().cloned()).unwrap_or(Val::Null),
            "join" => Val::Str(arg(0).as_list().unwrap_or(&[]).iter().map(Val::text).collect::<Vec<_>>().join(arg(1).as_str().unwrap_or(", "))),
            "upper" => Val::Str(arg(0).text().to_uppercase()),
            "lower" => Val::Str(arg(0).text().to_lowercase()),
            "json" => Val::Str(json(&arg(0))),
            "range" => {
                let (start, end, step) = if a.len() >= 2 { (n(0)?, n(1)?, arg(2).as_num().unwrap_or(1.0)) } else { (0.0, n(0)?, 1.0) };
                if step == 0.0 || !step.is_finite() {
                    return Err("range: the step can't be 0".into());
                }
                let mut out = vec![];
                let mut x = start;
                while (step > 0.0 && x < end) || (step < 0.0 && x > end) {
                    if out.len() >= MAX_LIST {
                        return Err(format!("range: more than {MAX_LIST} numbers"));
                    }
                    out.push(Val::Num(x));
                    x += step;
                }
                Val::List(out)
            }
            "String" => Val::Str(arg(0).text()),
            "Number" => Val::Num(arg(0).as_num().unwrap_or(f64::NAN)),
            "Boolean" => Val::Bool(arg(0).truthy()),
            _ => return Ok(None),
        }))
    }

    fn method(&mut self, v: &Val, name: &str, args: Vec<Val>) -> R<Val> {
        let arg = |i: usize| args.get(i).cloned().unwrap_or(Val::Null);
        let int = |i: usize, d: i64| args.get(i).and_then(Val::as_num).map(|n| n as i64).unwrap_or(d);
        match v {
            Val::List(l) => {
                let f = arg(0);
                Ok(match name {
                    "map" => {
                        let mut out = Vec::with_capacity(l.len());
                        for (i, x) in l.iter().enumerate() {
                            out.push(self.call_func(&f, vec![x.clone(), Val::Num(i as f64)])?);
                        }
                        Val::List(out)
                    }
                    "filter" => {
                        let mut out = vec![];
                        for (i, x) in l.iter().enumerate() {
                            if self.call_func(&f, vec![x.clone(), Val::Num(i as f64)])?.truthy() {
                                out.push(x.clone());
                            }
                        }
                        Val::List(out)
                    }
                    "find" | "findIndex" | "some" | "every" => {
                        for (i, x) in l.iter().enumerate() {
                            let hit = self.call_func(&f, vec![x.clone(), Val::Num(i as f64)])?.truthy();
                            match (name, hit) {
                                ("find", true) => return Ok(x.clone()),
                                ("findIndex", true) => return Ok(Val::Num(i as f64)),
                                ("some", true) => return Ok(Val::Bool(true)),
                                ("every", false) => return Ok(Val::Bool(false)),
                                _ => {}
                            }
                        }
                        match name {
                            "find" => Val::Null,
                            "findIndex" => Val::Num(-1.0),
                            "some" => Val::Bool(false),
                            _ => Val::Bool(true),
                        }
                    }
                    "reduce" => {
                        let mut it = l.iter().enumerate();
                        let mut acc = match args.get(1) {
                            Some(init) => init.clone(),
                            None => match it.next() {
                                Some((_, x)) => x.clone(),
                                None => return Err("reduce of an empty list needs a starting value".into()),
                            },
                        };
                        for (i, x) in it {
                            acc = self.call_func(&f, vec![acc, x.clone(), Val::Num(i as f64)])?;
                        }
                        acc
                    }
                    "sort" => {
                        let mut out = l.clone();
                        if matches!(f, Val::Func(_)) {
                            // Insertion sort: the comparator may fail, and lists here are short.
                            for i in 1..out.len() {
                                let mut j = i;
                                while j > 0 {
                                    let c = self.call_func(&f, vec![out[j - 1].clone(), out[j].clone()])?;
                                    if num(&c, "sort")? > 0.0 {
                                        out.swap(j - 1, j);
                                        j -= 1;
                                    } else {
                                        break;
                                    }
                                }
                            }
                        } else if out.iter().all(|x| matches!(x, Val::Num(_))) {
                            out.sort_by(|a, b| a.as_num().partial_cmp(&b.as_num()).unwrap_or(std::cmp::Ordering::Equal));
                        } else {
                            out.sort_by_key(|x| x.text());
                        }
                        Val::List(out)
                    }
                    "join" => Val::Str(l.iter().map(Val::text).collect::<Vec<_>>().join(args.first().map(|s| s.text()).as_deref().unwrap_or(","))),
                    "slice" => {
                        let (a, b) = slice_range(l.len(), int(0, 0), args.get(1).and_then(Val::as_num).map(|n| n as i64));
                        Val::List(l[a..b].to_vec())
                    }
                    "includes" => Val::Bool(l.contains(&f)),
                    "indexOf" => Val::Num(l.iter().position(|x| *x == f).map(|i| i as f64).unwrap_or(-1.0)),
                    "reverse" => Val::List(l.iter().rev().cloned().collect()),
                    "concat" => {
                        let mut out = l.clone();
                        for a in &args {
                            match a {
                                Val::List(m) => out.extend(m.iter().cloned()),
                                v => out.push(v.clone()),
                            }
                        }
                        Val::List(out)
                    }
                    "flat" => Val::List(l.iter().flat_map(|x| match x {
                        Val::List(m) => m.clone(),
                        v => vec![v.clone()],
                    }).collect()),
                    "at" => idx(l.len(), int(0, 0) as f64).and_then(|i| l.get(i).cloned()).unwrap_or(Val::Null),
                    "forEach" => Val::Null,
                    _ => return Err(format!("lists have no .{name}()")),
                })
            }
            Val::Str(s) => {
                let chars: Vec<char> = s.chars().collect();
                Ok(match name {
                    "toUpperCase" => Val::Str(s.to_uppercase()),
                    "toLowerCase" => Val::Str(s.to_lowercase()),
                    "trim" => Val::Str(s.trim().to_string()),
                    "split" => {
                        let sep = arg(0).text();
                        Val::List(if sep.is_empty() { chars.iter().map(|c| Val::Str(c.to_string())).collect() } else { s.split(sep.as_str()).map(|p| Val::Str(p.to_string())).collect() })
                    }
                    "includes" => Val::Bool(s.contains(arg(0).text().as_str())),
                    "startsWith" => Val::Bool(s.starts_with(arg(0).text().as_str())),
                    "endsWith" => Val::Bool(s.ends_with(arg(0).text().as_str())),
                    "indexOf" => Val::Num(s.find(arg(0).text().as_str()).map(|b| s[..b].chars().count() as f64).unwrap_or(-1.0)),
                    "replace" => Val::Str(s.replacen(arg(0).text().as_str(), &arg(1).text(), 1)),
                    "replaceAll" => Val::Str(s.replace(arg(0).text().as_str(), &arg(1).text())),
                    "slice" | "substring" => {
                        let (a, b) = slice_range(chars.len(), int(0, 0), args.get(1).and_then(Val::as_num).map(|n| n as i64));
                        Val::Str(chars[a..b].iter().collect())
                    }
                    "padStart" | "padEnd" => {
                        let want = int(0, 0).clamp(0, 1000) as usize;
                        let fill = args.get(1).map(Val::text).filter(|f| !f.is_empty()).unwrap_or_else(|| " ".into());
                        let mut pad = String::new();
                        while chars.len() + pad.chars().count() < want {
                            pad.push_str(&fill);
                        }
                        let pad: String = pad.chars().take(want.saturating_sub(chars.len())).collect();
                        Val::Str(if name == "padStart" { pad + s } else { s.clone() + &pad })
                    }
                    "repeat" => Val::Str(s.repeat(int(0, 0).clamp(0, 1000) as usize)),
                    "charAt" | "at" => idx(chars.len(), int(0, 0) as f64).map(|i| Val::Str(chars[i].to_string())).unwrap_or(Val::Str(String::new())),
                    "toString" => v.clone(),
                    _ => return Err(format!("texts have no .{name}()")),
                })
            }
            Val::Num(n) => Ok(match name {
                "toFixed" => Val::Str(format!("{:.*}", int(0, 0).clamp(0, 12) as usize, n)),
                "toString" => Val::Str(num_text(*n)),
                "toLocaleString" => Val::Str(fmt_num(*n, None)),
                _ => return Err(format!("numbers have no .{name}()")),
            }),
            Val::Null => Err(format!("can't call .{name}() on null")),
            v => Err(format!("{} has no .{name}()", v.type_name())),
        }
    }
}

fn idx(len: usize, n: f64) -> Option<usize> {
    if !n.is_finite() || n.fract() != 0.0 {
        return None;
    }
    let i = if n < 0.0 { len as i64 + n as i64 } else { n as i64 };
    (0..len as i64).contains(&i).then_some(i as usize)
}

fn slice_range(len: usize, a: i64, b: Option<i64>) -> (usize, usize) {
    let fix = |x: i64| if x < 0 { (len as i64 + x).max(0) as usize } else { (x as usize).min(len) };
    let (a, b) = (fix(a), fix(b.unwrap_or(len as i64)));
    (a, b.max(a))
}

fn num(v: &Val, op: &str) -> R<f64> {
    match v {
        Val::Null => Ok(0.0),
        v => v.as_num().ok_or_else(|| format!("{op} needs numbers, got {}", v.type_name())),
    }
}

fn member(v: &Val, name: &str) -> Val {
    match (v, name) {
        (Val::List(l), "length") => Val::Num(l.len() as f64),
        (Val::Str(s), "length") => Val::Num(s.chars().count() as f64),
        (Val::Obj(_), k) => v.field(k).clone(),
        (Val::Node(n), k) => n.named.iter().find(|(x, _)| x == k).map(|(_, v)| v.clone()).unwrap_or(Val::Null),
        _ => Val::Null,
    }
}

fn global_const(g: &str, name: &str) -> Option<Val> {
    Some(Val::Num(match (g, name) {
        ("Math", "PI") => std::f64::consts::PI,
        ("Math", "E") => std::f64::consts::E,
        ("Number", "MAX_SAFE_INTEGER") => 9007199254740991.0,
        _ => return None,
    }))
}

fn global_method(g: &str, name: &str, a: &[Val]) -> R<Val> {
    let n = |i: usize| num(a.get(i).unwrap_or(&Val::Null), name);
    let arg = |i: usize| a.get(i).cloned().unwrap_or(Val::Null);
    Ok(match (g, name) {
        ("Math", "round") => Val::Num((n(0)? + 0.5).floor()),
        ("Math", "floor") => Val::Num(n(0)?.floor()),
        ("Math", "ceil") => Val::Num(n(0)?.ceil()),
        ("Math", "trunc") => Val::Num(n(0)?.trunc()),
        ("Math", "abs") => Val::Num(n(0)?.abs()),
        ("Math", "sqrt") => Val::Num(n(0)?.sqrt()),
        ("Math", "sign") => Val::Num(n(0)?.signum()),
        ("Math", "log") => Val::Num(n(0)?.ln()),
        ("Math", "log10") => Val::Num(n(0)?.log10()),
        ("Math", "exp") => Val::Num(n(0)?.exp()),
        ("Math", "pow") => Val::Num(n(0)?.powf(n(1)?)),
        ("Math", "min") | ("Math", "max") => {
            let mut v = vec![];
            for x in a {
                v.push(num(x, name)?);
            }
            let r = if name == "min" { v.into_iter().reduce(f64::min) } else { v.into_iter().reduce(f64::max) };
            Val::Num(r.unwrap_or(if name == "min" { f64::INFINITY } else { f64::NEG_INFINITY }))
        }
        ("Object", "keys") => match arg(0) {
            Val::Obj(o) => Val::List(o.into_iter().map(|(k, _)| Val::Str(k)).collect()),
            _ => Val::List(vec![]),
        },
        ("Object", "values") => match arg(0) {
            Val::Obj(o) => Val::List(o.into_iter().map(|(_, v)| v).collect()),
            _ => Val::List(vec![]),
        },
        ("Object", "entries") => match arg(0) {
            Val::Obj(o) => Val::List(o.into_iter().map(|(k, v)| Val::List(vec![Val::Str(k), v])).collect()),
            _ => Val::List(vec![]),
        },
        ("JSON", "stringify") => Val::Str(json(&arg(0))),
        ("Number", "isFinite") => Val::Bool(arg(0).as_num().is_some_and(f64::is_finite)),
        ("Number", "parseFloat") | ("Number", "parseInt") => Val::Num(arg(0).as_num().map(|x| if name == "parseInt" { x.trunc() } else { x }).unwrap_or(f64::NAN)),
        ("Array", "isArray") => Val::Bool(matches!(arg(0), Val::List(_))),
        ("Array", "from") => match arg(0) {
            Val::List(l) => Val::List(l),
            Val::Str(s) => Val::List(s.chars().map(|c| Val::Str(c.to_string())).collect()),
            _ => Val::List(vec![]),
        },
        _ => return Err(format!("there's no {g}.{name}()")),
    })
}

fn binop(op: &str, a: &Val, b: &Val) -> R<Val> {
    Ok(match op {
        "+" => match (a, b) {
            (Val::Str(_), _) | (_, Val::Str(_)) | (Val::List(_), _) | (_, Val::List(_)) => Val::Str(a.text() + &b.text()),
            _ => Val::Num(num(a, "+")? + num(b, "+")?),
        },
        "-" => Val::Num(num(a, "-")? - num(b, "-")?),
        "*" => Val::Num(num(a, "*")? * num(b, "*")?),
        "/" => Val::Num(num(a, "/")? / num(b, "/")?),
        "%" => Val::Num(num(a, "%")? % num(b, "%")?),
        "**" => Val::Num(num(a, "**")?.powf(num(b, "**")?)),
        "==" | "===" => Val::Bool(loose_eq(a, b)),
        "!=" | "!==" => Val::Bool(!loose_eq(a, b)),
        "<" | "<=" | ">" | ">=" => {
            let ord = match (a, b) {
                (Val::Str(x), Val::Str(y)) => x.partial_cmp(y),
                _ => num(a, op)?.partial_cmp(&num(b, op)?),
            };
            let Some(o) = ord else { return Ok(Val::Bool(false)) };
            Val::Bool(match op {
                "<" => o.is_lt(),
                "<=" => o.is_le(),
                ">" => o.is_gt(),
                _ => o.is_ge(),
            })
        }
        _ => return Err(format!("unknown operator {op}")),
    })
}

/// `==`: values equal, and a number equals the same number written as text ("3" == 3).
fn loose_eq(a: &Val, b: &Val) -> bool {
    match (a, b) {
        (Val::Num(x), Val::Str(s)) | (Val::Str(s), Val::Num(x)) => s.trim().parse::<f64>().is_ok_and(|y| y == *x),
        _ => a == b,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The value of the last bare expression (not flattened for drawing).
    fn eval1(src: &str) -> Val {
        let p = Program::parse(src);
        let (st, errs) = p.initial_state();
        assert!(errs.is_empty(), "{errs:?}");
        let mut ev = Ev::new(&p, &st);
        let mut last = None;
        for (_, s) in &p.stmts {
            match s {
                Stmt::Root(e) => last = Some(ev.eval(e).unwrap_or_else(|e| panic!("{src}: {e}"))),
                Stmt::Bad(e) => panic!("{src}: {e}"),
                _ => {}
            }
        }
        match last {
            Some(v) => v,
            None => p.roots(&st).into_iter().next().expect("nothing to draw"),
        }
    }
    fn n(src: &str) -> f64 {
        eval1(src).as_num().unwrap_or_else(|| panic!("not a number: {src}"))
    }
    fn s(src: &str) -> String {
        eval1(src).text()
    }
    fn node(v: &Val) -> &Node {
        match v {
            Val::Node(n) => n,
            v => panic!("not a node: {v:?}"),
        }
    }

    #[test]
    fn arithmetic_and_precedence() {
        assert_eq!(n("1 + 2 * 3"), 7.0);
        assert_eq!(n("(1 + 2) * 3"), 9.0);
        assert_eq!(n("2 ** 3 ** 2"), 512.0);
        assert_eq!(n("-2 + 5"), 3.0);
        assert_eq!(n("10 % 4"), 2.0);
        assert_eq!(n("1_000 * 2"), 2000.0);
        assert_eq!(n("1.5e2"), 150.0);
        assert_eq!(s("1 + \"a\""), "1a");
        assert_eq!(s("0.1 + 0.2 > 0.3 ? \"yes\" : \"no\""), "yes");
    }

    #[test]
    fn logic_and_comparisons() {
        assert_eq!(eval1("1 < 2 && 2 <= 2"), Val::Bool(true));
        assert_eq!(eval1("null ?? 4"), Val::Num(4.0));
        assert_eq!(eval1("0 || \"x\""), Val::Str("x".into()));
        assert_eq!(eval1("!0"), Val::Bool(true));
        assert_eq!(eval1("\"3\" == 3"), Val::Bool(true));
        assert_eq!(eval1("[1, 2] == [1, 2]"), Val::Bool(true));
        assert_eq!(eval1("\"b\" > \"a\""), Val::Bool(true));
    }

    #[test]
    fn strings_and_templates() {
        assert_eq!(s("`Total: ${1 + 2} items`"), "Total: 3 items");
        assert_eq!(s("`a ${`b ${\"c\"}`} d`"), "a b c d");
        assert_eq!(s("'it\\'s'"), "it's");
        assert_eq!(s("\"\\u00e9t\\u{e9}\""), "été");
        assert_eq!(s("\"Hello\".toUpperCase() + \"!\".repeat(2)"), "HELLO!!");
        assert_eq!(s("\"a,b\".split(\",\").join(\" | \")"), "a | b");
        assert_eq!(s("(3.14159).toFixed(2)"), "3.14");
        assert_eq!(s("\"7\".padStart(3, \"0\")"), "007");
    }

    #[test]
    fn lists_objects_members() {
        assert_eq!(n("[1, 2, 3].length"), 3.0);
        assert_eq!(n("{a: {b: 5}}.a.b"), 5.0);
        assert_eq!(n("[10, 20, 30][1]"), 20.0);
        assert_eq!(n("{x: 1}[\"x\"]"), 1.0);
        assert_eq!(eval1("{}.missing.deeper"), Val::Null);
        assert_eq!(s("[...[1, 2], 3].join(\"-\")"), "1-2-3");
        assert_eq!(n("{...{a: 1, b: 2}, b: 3}.b"), 3.0);
        assert_eq!(s("a = 1\nb = 2\n{a, b}.b + \"\""), "2");
        assert_eq!(n("[1, 2, 3].at(-1)"), 3.0);
    }

    #[test]
    fn arrows_and_list_methods() {
        assert_eq!(s("[1, 2, 3].map(x => x * 2).join(\",\")"), "2,4,6");
        assert_eq!(n("[1, 2, 3, 4].filter(x => x % 2 == 0).length"), 2.0);
        assert_eq!(n("[{n: 1}, {n: 5}].find(r => r.n > 2).n"), 5.0);
        assert_eq!(n("[1, 2, 3].reduce((a, b) => a + b, 0)"), 6.0);
        assert_eq!(s("[3, 1, 2].sort().join()"), "1,2,3");
        assert_eq!(s("[3, 1, 2].sort((a, b) => b - a).join()"), "3,2,1");
        assert_eq!(s("[\"a\", \"b\"].map((x, i) => x + i).join()"), "a0,b1");
        assert_eq!(n("double = x => x * 2\ndouble(21)"), 42.0);
        assert_eq!(n("add = (a, b) => a + b\nadd(2, 3)"), 5.0);
        assert_eq!(n("k = 10\nf = x => x + k\nf(1)"), 11.0);
        assert_eq!(n("fact = n => n <= 1 ? 1 : n * fact(n - 1)\nfact(5)"), 120.0);
    }

    #[test]
    fn functions() {
        assert_eq!(n("round(2.345, 2)"), 2.35);
        assert_eq!(n("round(2.5)"), 3.0);
        assert_eq!(s("fmt(1234.5)"), "1,234.5");
        assert_eq!(s("fmt(1234567.891, 2)"), "1,234,567.89");
        assert_eq!(s("fmt(-0.5, 2)"), "-0.50");
        assert_eq!(s("fmt(290)"), "290");
        assert_eq!(n("sum([1, 2, 3.5])"), 6.5);
        assert_eq!(n("min([4, 2, 8])"), 2.0);
        assert_eq!(n("max(4, 2, 8)"), 8.0);
        assert_eq!(n("abs(-3)"), 3.0);
        assert_eq!(n("entries({a: 1, b: 2}).length"), 2.0);
        assert_eq!(n("Math.max(1, 9) + Math.round(1.4)"), 10.0);
        assert_eq!(n("Object.keys({a: 1, b: 2}).length"), 2.0);
        assert_eq!(n("range(5).length"), 5.0);
    }

    #[test]
    fn statements_state_and_continuation() {
        let p = Program::parse("$seats = 10\n$plan = \"Pro\"\nprice = $plan == \"Pro\" ? 29 : 12\nCard(\"Estimate\", [\n  Slider($seats, 1, 50, label: \"Seats\"),\n  Metric(\"Total\", \"$\" + fmt(price * $seats))\n])");
        let (mut st, errs) = p.initial_state();
        assert!(errs.is_empty());
        assert_eq!(st["seats"], Val::Num(10.0));
        let r = p.roots(&st);
        assert_eq!(r.len(), 1);
        let card = node(&r[0]);
        assert_eq!(card.name, "Card");
        assert_eq!(card.title(), "Estimate");
        let kids = card.kids();
        let slider = node(&kids[0]);
        assert_eq!(slider.bind(0), Some("seats"));
        assert_eq!(slider.str(4, "label"), "Seats");
        assert_eq!(node(&kids[1]).str(1, "value"), "$290");
        st.insert("seats".into(), Val::Num(2.0));
        let r2 = p.roots(&st);
        assert_eq!(node(&node(&r2[0]).kids()[1]).str(1, "value"), "$58");
        assert_eq!(shape(&r), shape(&r2), "values change, the layout doesn't");
        // An operator at the end of a line, or the start of the next, continues it.
        assert_eq!(n("1 +\n2"), 3.0);
        assert_eq!(n("x = 4\nx\n  * 2"), 8.0);
        assert_eq!(n("1; 2; 3 + 4".split("; ").last().unwrap()), 7.0);
        // Comments.
        assert_eq!(n("// a comment\n5 // five"), 5.0);
    }

    #[test]
    fn components_actions_and_paths() {
        let p = Program::parse("Card(\"A\", [Text(\"hi\"), Row([Badge(\"ok\", tone: \"good\"), null, false]), Button(\"Go\", Send(\"go\"))])\nDivider()");
        let st = p.initial_state().0;
        let r = p.roots(&st);
        assert_eq!(r.len(), 2);
        assert_eq!(shape(&r), "Card(Text,Row(Badge,),Button,);Divider;");
        let badge = at(&r, &[0, 1, 0]).unwrap();
        assert_eq!(node(&badge).tone(1), "good");
        let btn = at(&r, &[0, 2]).unwrap();
        let send = node(node(&btn).get(1, "action"));
        assert_eq!((send.name.as_str(), send.str(0, "text")), ("Send", "go".to_string()));
        assert!(at(&r, &[0, 9]).is_none());
        // Card with only children.
        let r = Program::parse("Card([Text(\"x\")])").roots(&State::new());
        assert_eq!(node(&r[0]).kids().len(), 1);
        assert_eq!(node(&r[0]).title(), "");
        // Unknown components are nodes too (drawn as a muted box).
        assert_eq!(node(&eval1("Sparkles(1, 2)")).name, "Sparkles");
        // `root` when there's no bare expression.
        assert_eq!(node(&eval1("root = Text(\"r\")")).name, "Text");
    }

    #[test]
    fn errors_stay_local_and_never_panic() {
        let p = Program::parse("Card(\"A\", [Text(nope), Text(\"fine\")])\nText(\"after\")\n)(\n\"unclosed");
        let r = p.roots(&p.initial_state().0);
        // The text shows its error inside the card; the next line still draws; the broken ones say why.
        let kids0 = node(&r[0]).kids();
        let bad = node(&kids0[0]);
        assert_eq!(bad.name, "Error");
        assert!(bad.str(0, "message").contains("nope isn't defined"), "{r:?}");
        assert_eq!(node(&node(&r[0]).kids()[1]).name, "Text");
        assert_eq!(node(&r[1]).name, "Text");
        assert!(r.iter().skip(2).all(|v| node(v).name == "Error"));
        assert!(r.len() >= 3);
        // Errors inside a list only spoil the component that holds it.
        let r = Program::parse("Stack([Text(\"ok\"), Metric(\"x\", 1 - {})])").roots(&State::new());
        let kids = node(&r[0]).kids();
        assert_eq!(node(&kids[0]).name, "Text");
        assert_eq!(node(&kids[1]).name, "Error");
        // Cycles, runaway recursion, huge ranges, undeclared state.
        for bad in ["a = b\nb = a\na", "f = x => f(x)\nf(1)", "range(1e9)", "$missing + 1", "[1, 2].nope()", "null.x()", "Text(", "1 +", "{a: }", "`${`", "@", "x = = 2"] {
            let p = Program::parse(bad);
            let r = p.roots(&p.initial_state().0);
            assert!(r.iter().any(|v| matches!(v, Val::Node(n) if n.name == "Error")), "{bad}: {r:?}");
        }
        // Garbage of every kind: no panic.
        for src in ["", "\n\n", "((((((", "]]]]", "$", "$1", "`", "\"\\u{zzzz}\"", "a.b.c(", "...x", "x => ", "() => 1", "{...}", "1e", "0.5.5", "Card(title: \"x\", children: [Text(\"y\")])"] {
            let p = Program::parse(src);
            let _ = p.roots(&p.initial_state().0);
        }
    }

    #[test]
    fn named_children_and_state_errors() {
        let r = Program::parse("Card(title: \"T\", children: [Text(\"y\")])").roots(&State::new());
        assert_eq!(node(&r[0]).title(), "T");
        assert_eq!(node(&r[0]).kids().len(), 1);
        let p = Program::parse("$x = 1 - {}\nText(\"t\")");
        let (st, errs) = p.initial_state();
        assert_eq!(st["x"], Val::Null);
        assert_eq!(errs.len(), 1);
    }
}
