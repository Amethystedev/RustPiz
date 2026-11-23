//! Expressions `{{…}}` des styles : analyse et évaluation.

use serde::{Deserialize, Serialize};

/// Valeur manipulée par une expression.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Value {
    Num(f64),
    Str(String),
}

impl Value {
    /// Rendu textuel invariant, tel que XAML l'attend.
    pub fn render(&self) -> String {
        match self {
            Value::Str(text) => text.clone(),
            Value::Num(n) if n.is_nan() => "NaN".to_owned(),
            Value::Num(n) if n.is_infinite() => {
                if *n < 0.0 { "-Infinity" } else { "Infinity" }.to_owned()
            }
            Value::Num(n) => n.to_string(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Lt,
    Le,
    Gt,
    Ge,
    Eq,
    Ne,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    Num(f64),
    Str(String),
    Var(String),
    Neg(Box<Expr>),
    Plus(Box<Expr>),
    Bin(BinOp, Box<Expr>, Box<Expr>),
    Cond(Box<Expr>, Box<Expr>, Box<Expr>),
    Min(Box<Expr>, Box<Expr>),
    Max(Box<Expr>, Box<Expr>),
    Skip,
}

/// Raison pour laquelle une valeur n'a pas pu être produite.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EvalError {
    /// `skip()` : la propriété garde ou retrouve sa valeur d'origine.
    Skip,
    /// Variable absente dans une substitution directe.
    Undefined(String),
    /// Opérande du mauvais type.
    Type(&'static str),
}

const MAX_DEPTH: usize = 64;

struct Parser<'a> {
    text: &'a str,
    pos: usize,
    depth: usize,
}

type ParseResult = Result<Expr, String>;

impl<'a> Parser<'a> {
    fn peek(&self) -> Option<char> {
        self.text[self.pos..].chars().next()
    }

    fn skip_ws(&mut self) {
        while matches!(self.peek(), Some(' ' | '\t' | '\r' | '\n')) {
            self.pos += 1;
        }
    }

    fn eat(&mut self, token: &str) -> bool {
        self.skip_ws();
        if self.text[self.pos..].starts_with(token) {
            self.pos += token.len();
            true
        } else {
            false
        }
    }

    fn expect(&mut self, token: &str) -> Result<(), String> {
        if self.eat(token) {
            Ok(())
        } else {
            Err(format!("'{token}' attendu à la position {}", self.pos))
        }
    }

    fn conditional(&mut self) -> ParseResult {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err("expression trop imbriquée".to_owned());
        }
        let condition = self.equality()?;
        let result = if self.eat("?") {
            let yes = self.conditional()?;
            self.expect(":")?;
            let no = self.conditional()?;
            Expr::Cond(Box::new(condition), Box::new(yes), Box::new(no))
        } else {
            condition
        };
        self.depth -= 1;
        Ok(result)
    }

    fn equality(&mut self) -> ParseResult {
        let mut left = self.relational()?;
        loop {
            let op = if self.eat("==") {
                BinOp::Eq
            } else if self.eat("!=") {
                BinOp::Ne
            } else {
                return Ok(left);
            };
            left = Expr::Bin(op, Box::new(left), Box::new(self.relational()?));
        }
    }

    fn relational(&mut self) -> ParseResult {
        let mut left = self.additive()?;
        loop {
            let op = if self.eat("<=") {
                BinOp::Le
            } else if self.eat(">=") {
                BinOp::Ge
            } else if self.eat("<") {
                BinOp::Lt
            } else if self.eat(">") {
                BinOp::Gt
            } else {
                return Ok(left);
            };
            left = Expr::Bin(op, Box::new(left), Box::new(self.additive()?));
        }
    }

    fn additive(&mut self) -> ParseResult {
        let mut left = self.multiplicative()?;
        loop {
            let op = if self.eat("+") {
                BinOp::Add
            } else if self.eat("-") {
                BinOp::Sub
            } else {
                return Ok(left);
            };
            left = Expr::Bin(op, Box::new(left), Box::new(self.multiplicative()?));
        }
    }

    fn multiplicative(&mut self) -> ParseResult {
        let mut left = self.unary()?;
        loop {
            let op = if self.eat("*") {
                BinOp::Mul
            } else if self.eat("/") {
                BinOp::Div
            } else {
                return Ok(left);
            };
            left = Expr::Bin(op, Box::new(left), Box::new(self.unary()?));
        }
    }

    fn unary(&mut self) -> ParseResult {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err("expression trop imbriquée".to_owned());
        }
        let result = if self.eat("+") {
            Expr::Plus(Box::new(self.unary()?))
        } else if self.eat("-") {
            Expr::Neg(Box::new(self.unary()?))
        } else {
            self.primary()?
        };
        self.depth -= 1;
        Ok(result)
    }

    fn primary(&mut self) -> ParseResult {
        self.skip_ws();
        match self.peek() {
            Some('(') => {
                self.pos += 1;
                let inner = self.conditional()?;
                self.expect(")")?;
                Ok(inner)
            }
            Some('`') => self.string(),
            Some(c) if c.is_ascii_digit() || c == '.' => self.number(),
            Some(c) if c.is_ascii_alphabetic() || c == '_' => self.identifier(),
            other => Err(format!(
                "jeton inattendu {other:?} à la position {}",
                self.pos
            )),
        }
    }

    fn string(&mut self) -> ParseResult {
        self.pos += 1;
        let mut out = String::new();
        loop {
            let Some(c) = self.peek() else {
                return Err("chaîne non terminée".to_owned());
            };
            self.pos += c.len_utf8();
            if c != '`' {
                out.push(c);
            } else if self.peek() == Some('`') {
                self.pos += 1;
                out.push('`');
            } else {
                return Ok(Expr::Str(out));
            }
        }
    }

    fn number(&mut self) -> ParseResult {
        let start = self.pos;
        let mut seen_dot = false;
        while let Some(c) = self.peek() {
            if c.is_ascii_digit() || (c == '.' && !seen_dot) {
                seen_dot |= c == '.';
                self.pos += 1;
            } else {
                break;
            }
        }
        if matches!(self.peek(), Some('e' | 'E')) {
            let mark = self.pos;
            self.pos += 1;
            if matches!(self.peek(), Some('+' | '-')) {
                self.pos += 1;
            }
            let digits = self.pos;
            while matches!(self.peek(), Some(c) if c.is_ascii_digit()) {
                self.pos += 1;
            }
            if digits == self.pos {
                self.pos = mark;
            }
        }
        let text = &self.text[start..self.pos];
        text.parse()
            .map(Expr::Num)
            .map_err(|_| format!("nombre invalide : {text}"))
    }

    fn identifier(&mut self) -> ParseResult {
        let start = self.pos;
        while matches!(self.peek(), Some(c) if c.is_ascii_alphanumeric() || c == '_') {
            self.pos += 1;
        }
        let name = &self.text[start..self.pos];
        if self.peek() != Some('(') {
            return Ok(Expr::Var(name.to_owned()));
        }
        self.pos += 1;
        if name == "skip" {
            self.expect(")")?;
            return Ok(Expr::Skip);
        }
        if name != "min" && name != "max" {
            return Err(format!("fonction inconnue : {name}"));
        }
        let first = Box::new(self.conditional()?);
        self.expect(",")?;
        let second = Box::new(self.conditional()?);
        self.expect(")")?;
        Ok(if name == "min" {
            Expr::Min(first, second)
        } else {
            Expr::Max(first, second)
        })
    }
}

/// Analyse le contenu d'une expression, sans les accolades.
pub fn parse_expression(text: &str) -> Result<Expr, String> {
    let mut parser = Parser {
        text,
        pos: 0,
        depth: 0,
    };
    let expr = parser.conditional()?;
    parser.skip_ws();
    if parser.pos != text.len() {
        return Err(format!("texte en trop à la position {}", parser.pos));
    }
    Ok(expr)
}

fn number(value: Value) -> Result<f64, EvalError> {
    match value {
        Value::Num(n) => Ok(n),
        Value::Str(_) => Err(EvalError::Type("opérande numérique attendu")),
    }
}

fn truth(condition: bool) -> Value {
    Value::Num(f64::from(u8::from(condition)))
}

/// Évalue une expression. Une variable absente vaut la chaîne vide.
pub fn evaluate(
    expr: &Expr,
    lookup: &mut dyn FnMut(&str) -> Option<Value>,
) -> Result<Value, EvalError> {
    Ok(match expr {
        Expr::Num(n) => Value::Num(*n),
        Expr::Str(text) => Value::Str(text.clone()),
        Expr::Var(name) => lookup(name).unwrap_or(Value::Str(String::new())),
        Expr::Skip => return Err(EvalError::Skip),
        Expr::Plus(inner) => Value::Num(number(evaluate(inner, lookup)?)?),
        Expr::Neg(inner) => Value::Num(-number(evaluate(inner, lookup)?)?),
        Expr::Cond(condition, yes, no) => {
            if number(evaluate(condition, lookup)?)? != 0.0 {
                evaluate(yes, lookup)?
            } else {
                evaluate(no, lookup)?
            }
        }
        Expr::Min(a, b) => {
            let (a, b) = (number(evaluate(a, lookup)?)?, number(evaluate(b, lookup)?)?);
            Value::Num(a.min(b))
        }
        Expr::Max(a, b) => {
            let (a, b) = (number(evaluate(a, lookup)?)?, number(evaluate(b, lookup)?)?);
            Value::Num(a.max(b))
        }
        Expr::Bin(op, left, right) => {
            let left = evaluate(left, lookup)?;
            let right = evaluate(right, lookup)?;
            match op {
                BinOp::Eq => truth(left == right),
                BinOp::Ne => truth(left != right),
                _ => {
                    let (a, b) = (number(left)?, number(right)?);
                    match op {
                        BinOp::Add => Value::Num(a + b),
                        BinOp::Sub => Value::Num(a - b),
                        BinOp::Mul => Value::Num(a * b),
                        BinOp::Div => Value::Num(a / b),
                        BinOp::Lt => truth(a < b),
                        BinOp::Le => truth(a <= b),
                        BinOp::Gt => truth(a > b),
                        BinOp::Ge => truth(a >= b),
                        BinOp::Eq | BinOp::Ne => unreachable!("traités plus haut"),
                    }
                }
            }
        }
    })
}

/// Liste les variables référencées par une expression.
pub fn collect_variables(expr: &Expr, out: &mut Vec<String>) {
    match expr {
        Expr::Var(name) => out.push(name.clone()),
        Expr::Neg(a) | Expr::Plus(a) => collect_variables(a, out),
        Expr::Bin(_, a, b) | Expr::Min(a, b) | Expr::Max(a, b) => {
            collect_variables(a, out);
            collect_variables(b, out);
        }
        Expr::Cond(a, b, c) => {
            collect_variables(a, out);
            collect_variables(b, out);
            collect_variables(c, out);
        }
        Expr::Num(_) | Expr::Str(_) | Expr::Skip => {}
    }
}

/// Morceau d'une valeur contenant des expressions.
#[derive(Debug, Clone, PartialEq)]
pub enum Part {
    Text(String),
    Expr(Expr),
}

/// Valeur découpée en texte littéral et expressions.
#[derive(Debug, Clone, PartialEq)]
pub struct Template(pub Vec<Part>);

impl Template {
    /// Découpe une valeur. Les paires d'accolades s'apparient depuis l'intérieur.
    pub fn parse(value: &str) -> Result<Self, String> {
        let mut parts = Vec::new();
        let mut rest = value;
        loop {
            let found = rest
                .find("}}")
                .and_then(|close| rest[..close].rfind("{{").map(|open| (open, close)));
            let Some((open, close)) = found else {
                if !rest.is_empty() {
                    parts.push(Part::Text(rest.to_owned()));
                }
                return Ok(Template(parts));
            };
            if open > 0 {
                parts.push(Part::Text(rest[..open].to_owned()));
            }
            parts.push(Part::Expr(parse_expression(&rest[open + 2..close])?));
            rest = &rest[close + 2..];
        }
    }

    /// Variables référencées, sans doublons.
    pub fn variables(&self) -> Vec<String> {
        let mut names = Vec::new();
        for part in &self.0 {
            if let Part::Expr(expr) = part {
                collect_variables(expr, &mut names);
            }
        }
        names.sort();
        names.dedup();
        names
    }

    /// Produit la valeur finale.
    pub fn render(
        &self,
        lookup: &mut dyn FnMut(&str) -> Option<Value>,
    ) -> Result<String, EvalError> {
        let mut out = String::new();
        for part in &self.0 {
            match part {
                Part::Text(text) => out.push_str(text),
                Part::Expr(Expr::Var(name)) => match lookup(name) {
                    Some(value) => out.push_str(&value.render()),
                    None => return Err(EvalError::Undefined(name.clone())),
                },
                Part::Expr(expr) => out.push_str(&evaluate(expr, lookup)?.render()),
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render(value: &str, vars: &[(&str, Value)]) -> Result<String, EvalError> {
        Template::parse(value).unwrap().render(&mut |name| {
            vars.iter()
                .find(|(n, _)| *n == name)
                .map(|(_, v)| v.clone())
        })
    }

    #[test]
    fn surface_expressions() {
        let tpl = "{{taskbarDock==`vertical`?skip():70}}";
        let horizontal = [("taskbarDock", Value::Str("horizontal".into()))];
        let vertical = [("taskbarDock", Value::Str("vertical".into()))];
        assert_eq!(render(tpl, &horizontal).unwrap(), "70");
        assert_eq!(render(tpl, &vertical), Err(EvalError::Skip));
        assert_eq!(
            render("{{taskbarDock==`vertical`?skip():`Auto`}}", &horizontal).unwrap(),
            "Auto"
        );
    }

    #[test]
    fn arithmetic_precedence_and_functions() {
        let vars = [("a", Value::Num(3.0))];
        assert_eq!(render("{{1 + 2 * a}}", &vars).unwrap(), "7");
        assert_eq!(render("{{(1 + 2) * a}}", &vars).unwrap(), "9");
        assert_eq!(render("{{min(a, 2) * max(1, 4)}}", &vars).unwrap(), "8");
        assert_eq!(render("{{-a + 1}}", &vars).unwrap(), "-2");
        assert_eq!(render("{{a >= 3}}", &vars).unwrap(), "1");
    }

    #[test]
    fn embedded_substitutions_and_braces() {
        let vars = [("x", Value::Num(1.5))];
        assert_eq!(
            render("0,{{x}},0,{{x + 10}}", &vars).unwrap(),
            "0,1.5,0,11.5"
        );
        assert_eq!(render("{{{x}}}", &vars).unwrap(), "{1.5}");
    }

    #[test]
    fn undefined_variable_rules() {
        assert_eq!(
            render("{{w}}", &[]),
            Err(EvalError::Undefined("w".to_owned()))
        );
        assert_eq!(render("{{w == `` ? 80 : w}}", &[]).unwrap(), "80");
        assert!(matches!(render("{{w + 1}}", &[]), Err(EvalError::Type(_))));
    }

    #[test]
    fn string_escapes_and_mixed_equality() {
        assert_eq!(render("{{`a``b`}}", &[]).unwrap(), "a`b");
        assert_eq!(render("{{1 == `1`}}", &[]).unwrap(), "0");
    }

    #[test]
    fn syntax_errors_are_reported() {
        for bad in ["{{1 +}}", "{{foo(1)}}", "{{(1}}", "{{`x}}", "{{1 2}}"] {
            assert!(Template::parse(bad).is_err(), "devrait échouer : {bad}");
        }
    }

    #[test]
    fn nesting_is_bounded() {
        let deep = format!("{{{{{}1{}}}}}", "(".repeat(5000), ")".repeat(5000));
        assert!(Template::parse(&deep).is_err());
        let unary = format!("{{{{{}1}}}}", "-".repeat(5000));
        assert!(Template::parse(&unary).is_err());
    }

    #[test]
    fn variables_are_listed() {
        let tpl = Template::parse("{{a + b}}x{{a ? c : 1}}").unwrap();
        assert_eq!(tpl.variables(), ["a", "b", "c"]);
    }
}
