//! 公式引擎：表达式 DSL（算术 / 比较 / 逻辑 / 函数 / 字段引用）+ 求值 + computed 列 + 批量填充。
//!
//! 公式不写进数据文件 —— computed 列在导出时物化；`apply_formula` 用于一次性批量填充。

use crate::types::{TypeInfo, TypeKind};
use crate::value::{DType, TableData};

// ============================================================================
// AST
// ============================================================================

#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    Num(f64),
    Str(String),
    /// 字段引用（当前行的字段名）
    Field(String),
    /// 一元：'-'（负）/ '!'（非）
    Unary(char, Box<Expr>),
    /// 二元：+ - * / % ^ == != < <= > >= && ||
    Binary(String, Box<Expr>, Box<Expr>),
    /// 函数调用
    Call(String, Vec<Expr>),
    /// 三元 cond ? a : b
    Ternary(Box<Expr>, Box<Expr>, Box<Expr>),
}

// ============================================================================
// Tokenizer + Parser
// ============================================================================

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Num(f64),
    Str(String),
    Ident(String),
    Op(String),
    LParen,
    RParen,
    Question,
    Colon,
}

fn tokenize(s: &str) -> Result<Vec<Tok>, String> {
    let mut toks = Vec::new();
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            ' ' | '\t' | '\n' | '\r' => i += 1,
            '(' => {
                toks.push(Tok::LParen);
                i += 1;
            }
            ')' => {
                toks.push(Tok::RParen);
                i += 1;
            }
            '?' => {
                toks.push(Tok::Question);
                i += 1;
            }
            ':' => {
                toks.push(Tok::Colon);
                i += 1;
            }
            '"' => {
                let mut s2 = String::new();
                i += 1;
                while i < chars.len() && chars[i] != '"' {
                    s2.push(chars[i]);
                    i += 1;
                }
                if i >= chars.len() {
                    return Err("未闭合的字符串".into());
                }
                i += 1;
                toks.push(Tok::Str(s2));
            }
            '0'..='9' | '.' => {
                let start = i;
                while i < chars.len()
                    && (chars[i].is_ascii_digit() || chars[i] == '.' || chars[i] == '_')
                {
                    i += 1;
                }
                let num_str: String = chars[start..i].iter().collect::<String>().replace('_', "");
                let v: f64 = num_str
                    .parse()
                    .map_err(|_| format!("非法数字 '{}'", num_str))?;
                toks.push(Tok::Num(v));
            }
            '+' | '-' | '*' | '/' | '%' | '^' | '<' | '>' | '=' | '!' | '&' | '|' | ',' => {
                // 双字符运算符
                let two: String = chars[i..(i + 2).min(chars.len())].iter().collect();
                if matches!(two.as_str(), "==" | "!=" | "<=" | ">=" | "&&" | "||") {
                    toks.push(Tok::Op(two));
                    i += 2;
                } else {
                    toks.push(Tok::Op(c.to_string()));
                    i += 1;
                }
            }
            _ => {
                // 标识符（字段名 / 函数名）
                let start = i;
                while i < chars.len()
                    && !matches!(
                        chars[i],
                        ' ' | '\t'
                            | '('
                            | ')'
                            | '?'
                            | ':'
                            | '"'
                            | '+'
                            | '-'
                            | '*'
                            | '/'
                            | '%'
                            | '^'
                            | '<'
                            | '>'
                            | '='
                            | '!'
                            | '&'
                            | '|'
                    )
                {
                    i += 1;
                }
                if start == i {
                    return Err(format!("非法字符 '{}'", c));
                }
                let ident: String = chars[start..i].iter().collect();
                toks.push(Tok::Ident(ident));
            }
        }
    }
    Ok(toks)
}

struct Parser {
    toks: Vec<Tok>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos)
    }
    fn next(&mut self) -> Option<Tok> {
        let t = self.toks.get(self.pos).cloned();
        self.pos += 1;
        t
    }
    fn expect_op(&mut self, op: &str) -> bool {
        if let Some(Tok::Op(o)) = self.peek()
            && o == op
        {
            self.pos += 1;
            return true;
        }
        false
    }

    // expr := ternary
    fn expr(&mut self) -> Result<Expr, String> {
        self.ternary()
    }

    fn ternary(&mut self) -> Result<Expr, String> {
        let cond = self.or()?;
        if self.expect_op("?") {
            let a = self.expr()?;
            if !matches!(self.next(), Some(Tok::Colon)) {
                return Err("三元表达式缺少 ':'".into());
            }
            let b = self.expr()?;
            return Ok(Expr::Ternary(Box::new(cond), Box::new(a), Box::new(b)));
        }
        Ok(cond)
    }

    fn or(&mut self) -> Result<Expr, String> {
        let mut left = self.and()?;
        while self.expect_op("||") {
            let right = self.and()?;
            left = Expr::Binary("||".into(), Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn and(&mut self) -> Result<Expr, String> {
        let mut left = self.cmp()?;
        while self.expect_op("&&") {
            let right = self.cmp()?;
            left = Expr::Binary("&&".into(), Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn cmp(&mut self) -> Result<Expr, String> {
        let mut left = self.add()?;
        for op in ["==", "!=", "<=", ">=", "<", ">"] {
            if self.expect_op(op) {
                let right = self.add()?;
                left = Expr::Binary(op.to_string(), Box::new(left), Box::new(right));
                return Ok(left);
            }
        }
        Ok(left)
    }

    fn add(&mut self) -> Result<Expr, String> {
        let mut left = self.mul()?;
        loop {
            if self.expect_op("+") {
                let right = self.mul()?;
                left = Expr::Binary("+".into(), Box::new(left), Box::new(right));
            } else if self.expect_op("-") {
                let right = self.mul()?;
                left = Expr::Binary("-".into(), Box::new(left), Box::new(right));
            } else {
                break;
            }
        }
        Ok(left)
    }

    fn mul(&mut self) -> Result<Expr, String> {
        let mut left = self.unary()?;
        loop {
            if self.expect_op("*") {
                let right = self.unary()?;
                left = Expr::Binary("*".into(), Box::new(left), Box::new(right));
            } else if self.expect_op("/") {
                let right = self.unary()?;
                left = Expr::Binary("/".into(), Box::new(left), Box::new(right));
            } else if self.expect_op("%") {
                let right = self.unary()?;
                left = Expr::Binary("%".into(), Box::new(left), Box::new(right));
            } else {
                break;
            }
        }
        Ok(left)
    }

    fn unary(&mut self) -> Result<Expr, String> {
        if self.expect_op("-") {
            let e = self.unary()?;
            return Ok(Expr::Unary('-', Box::new(e)));
        }
        if self.expect_op("!") {
            let e = self.unary()?;
            return Ok(Expr::Unary('!', Box::new(e)));
        }
        self.power()
    }

    fn power(&mut self) -> Result<Expr, String> {
        let base = self.primary()?;
        if self.expect_op("^") {
            let exp = self.unary()?;
            return Ok(Expr::Binary("^".into(), Box::new(base), Box::new(exp)));
        }
        Ok(base)
    }

    fn primary(&mut self) -> Result<Expr, String> {
        match self.next() {
            Some(Tok::Num(n)) => Ok(Expr::Num(n)),
            Some(Tok::Str(s)) => Ok(Expr::Str(s)),
            Some(Tok::LParen) => {
                let e = self.expr()?;
                if !matches!(self.next(), Some(Tok::RParen)) {
                    return Err("缺少 ')'".into());
                }
                Ok(e)
            }
            Some(Tok::Ident(name)) => {
                // 函数调用？
                if matches!(self.peek(), Some(Tok::LParen)) {
                    self.pos += 1; // 消费 (
                    let mut args = Vec::new();
                    if !matches!(self.peek(), Some(Tok::RParen)) {
                        loop {
                            args.push(self.expr()?);
                            if self.expect_op(",") {
                                continue;
                            }
                            break;
                        }
                    }
                    if !matches!(self.next(), Some(Tok::RParen)) {
                        return Err("函数调用缺少 ')'".into());
                    }
                    Ok(Expr::Call(name, args))
                } else {
                    Ok(Expr::Field(name))
                }
            }
            Some(t) => Err(format!("意外的 token: {:?}", t)),
            None => Err("表达式不完整".into()),
        }
    }
}

/// 解析表达式字符串。
pub fn parse_expr(s: &str) -> Result<Expr, String> {
    let toks = tokenize(s)?;
    let mut parser = Parser { toks, pos: 0 };
    let e = parser.expr()?;
    if parser.pos != parser.toks.len() {
        return Err("表达式末尾有多余内容".into());
    }
    Ok(e)
}

// ============================================================================
// 求值
// ============================================================================

/// 环境：字段名 → 数值。
pub trait EvalEnv {
    fn lookup(&self, field: &str) -> Option<f64>;
}

/// 求值表达式（结果为数值）。
pub fn eval(expr: &Expr, env: &dyn EvalEnv) -> Result<f64, String> {
    match expr {
        Expr::Num(n) => Ok(*n),
        Expr::Str(_) => Err("字符串不能参与数值运算".into()),
        Expr::Field(name) => env
            .lookup(name)
            .ok_or_else(|| format!("未找到字段 '{}'", name)),
        Expr::Unary('-', e) => Ok(-eval(e, env)?),
        Expr::Unary('!', e) => Ok(if eval(e, env)? != 0.0 { 0.0 } else { 1.0 }),
        Expr::Binary(op, a, b) => {
            let x = eval(a, env)?;
            let y = eval(b, env)?;
            match op.as_str() {
                "+" => Ok(x + y),
                "-" => Ok(x - y),
                "*" => Ok(x * y),
                "/" => {
                    if y == 0.0 {
                        Err("除以零".into())
                    } else {
                        Ok(x / y)
                    }
                }
                "%" => Ok(x % y),
                "^" => Ok(x.powf(y)),
                "==" => Ok((x == y) as i32 as f64),
                "!=" => Ok((x != y) as i32 as f64),
                "<" => Ok((x < y) as i32 as f64),
                "<=" => Ok((x <= y) as i32 as f64),
                ">" => Ok((x > y) as i32 as f64),
                ">=" => Ok((x >= y) as i32 as f64),
                "&&" => Ok(((x != 0.0) && (y != 0.0)) as i32 as f64),
                "||" => Ok(((x != 0.0) || (y != 0.0)) as i32 as f64),
                _ => Err(format!("未知运算符 '{}'", op)),
            }
        }
        Expr::Ternary(c, a, b) => {
            if eval(c, env)? != 0.0 {
                eval(a, env)
            } else {
                eval(b, env)
            }
        }
        Expr::Call(name, args) => {
            let vals = args
                .iter()
                .map(|a| eval(a, env))
                .collect::<Result<Vec<_>, _>>()?;
            call_function(name, &vals)
        }
        Expr::Unary(_, _) => Err("未知一元运算符".into()),
    }
}

fn call_function(name: &str, args: &[f64]) -> Result<f64, String> {
    match name {
        "min" => args.iter().copied().fold(f64::INFINITY, f64::min).pipe_ok(),
        "max" => args
            .iter()
            .copied()
            .fold(f64::NEG_INFINITY, f64::max)
            .pipe_ok(),
        "abs" => one_arg(args).map(|x| x.abs()),
        "round" => one_arg(args).map(|x| x.round()),
        "floor" => one_arg(args).map(|x| x.floor()),
        "ceil" => one_arg(args).map(|x| x.ceil()),
        "if" => {
            if args.len() < 2 {
                Err("if 需至少 2 个参数".into())
            } else if args[0] != 0.0 {
                Ok(args[1])
            } else if args.len() >= 3 {
                Ok(args[2])
            } else {
                Ok(0.0)
            }
        }
        _ => Err(format!("未知函数 '{}'", name)),
    }
}

fn one_arg(args: &[f64]) -> Result<f64, String> {
    if args.len() != 1 {
        Err(format!("需 1 个参数，实际 {} 个", args.len()))
    } else {
        Ok(args[0])
    }
}

trait PipeOk: Sized {
    fn pipe_ok(self) -> Result<Self, String>;
}
impl PipeOk for f64 {
    fn pipe_ok(self) -> Result<Self, String> {
        if self.is_finite() {
            Ok(self)
        } else {
            Err("函数结果非有限值".into())
        }
    }
}

// ============================================================================
// computed 列 + apply_formula
// ============================================================================

/// computed 列定义（列级公式，不落盘）。
#[derive(Debug, Clone)]
pub struct ComputedColumn {
    pub field: String,
    pub type_str: String,
    pub expr: String,
}

/// 从当前行构建求值环境。
struct RowEnv<'a> {
    fields: &'a [(String, TypeInfo)],
    data: &'a [DType],
}

impl EvalEnv for RowEnv<'_> {
    fn lookup(&self, field: &str) -> Option<f64> {
        let pos = self.fields.iter().position(|(n, _)| n == field)?;
        let v = self.data.get(pos)?;
        match v {
            DType::Int(i) => Some(*i as f64),
            DType::UInt(u) => Some(*u as f64),
            DType::Float(f) => Some(*f),
            DType::Bool(b) => Some(*b as i32 as f64),
            _ => None,
        }
    }
}

/// 批量填充：对每行求值 expr，写回 target 字段。返回更新的行数。
pub fn apply_formula(
    data: &mut TableData,
    fields: &[(String, TypeInfo)],
    target: &str,
    expr_str: &str,
) -> Result<usize, String> {
    let expr = parse_expr(expr_str)?;
    let target_pos = fields
        .iter()
        .position(|(n, _)| n == target)
        .ok_or_else(|| format!("目标字段 '{}' 不存在", target))?;

    let mut updated = 0;
    for record in &mut data.records {
        let env = RowEnv {
            fields,
            data: &record.data,
        };
        let val = eval(&expr, &env)?;
        // 写回（根据目标字段类型）
        let target_ti = &fields[target_pos].1;
        let dv = match target_ti.kind {
            TypeKind::F32 | TypeKind::F64 => DType::Float(val),
            _ => DType::Int(val.round() as i64),
        };
        if record.data.len() > target_pos {
            record.data[target_pos] = dv;
            updated += 1;
        }
    }
    Ok(updated)
}

/// 计算 computed 列的值（返回 (field, value) 列表，供导出物化）。
pub fn compute_columns(
    record_data: &[DType],
    fields: &[(String, TypeInfo)],
    columns: &[ComputedColumn],
) -> Result<Vec<(String, DType)>, String> {
    let env = RowEnv {
        fields,
        data: record_data,
    };
    let mut out = Vec::new();
    for col in columns {
        let expr = parse_expr(&col.expr)?;
        let val = eval(&expr, &env)?;
        let dv = if col.type_str.contains("f32") || col.type_str.contains("f64") {
            DType::Float(val)
        } else {
            DType::Int(val.round() as i64)
        };
        out.push((col.field.clone(), dv));
    }
    Ok(out)
}

// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    struct Env(HashMap<String, f64>);
    impl EvalEnv for Env {
        fn lookup(&self, field: &str) -> Option<f64> {
            self.0.get(field).copied()
        }
    }

    fn env(pairs: &[(&str, f64)]) -> Env {
        Env(pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect())
    }

    #[test]
    fn parse_and_eval_arithmetic() {
        let e = parse_expr("price * count * (1 - discount)").unwrap();
        let v = eval(
            &e,
            &env(&[("price", 10.0), ("count", 2.0), ("discount", 0.1)]),
        )
        .unwrap();
        assert_eq!(v, 18.0);
    }

    #[test]
    fn eval_functions() {
        assert_eq!(
            eval(&parse_expr("min(3, 7)").unwrap(), &env(&[])).unwrap(),
            3.0
        );
        assert_eq!(
            eval(&parse_expr("max(3, 7)").unwrap(), &env(&[])).unwrap(),
            7.0
        );
        assert_eq!(
            eval(&parse_expr("if(1 > 0, 100, 200)").unwrap(), &env(&[])).unwrap(),
            100.0
        );
        assert_eq!(
            eval(&parse_expr("round(3.6)").unwrap(), &env(&[])).unwrap(),
            4.0
        );
    }

    #[test]
    fn eval_comparison_and_logic() {
        assert_eq!(eval(&parse_expr("1 < 2").unwrap(), &env(&[])).unwrap(), 1.0);
        assert_eq!(
            eval(&parse_expr("(1 < 2) && (3 > 4)").unwrap(), &env(&[])).unwrap(),
            0.0
        );
    }

    #[test]
    fn apply_formula_to_all_rows() {
        let fields = vec![
            ("price".to_string(), TypeInfo::new(TypeKind::I32)),
            ("count".to_string(), TypeInfo::new(TypeKind::I32)),
            ("total".to_string(), TypeInfo::new(TypeKind::I32)),
        ];
        let mut data = TableData::new();
        for (p, c) in [(10, 2), (20, 3)] {
            let mut r = crate::Record::new();
            r.data = vec![DType::Int(p), DType::Int(c), DType::Int(0)];
            data.push(r);
        }
        let updated = apply_formula(&mut data, &fields, "total", "price * count").unwrap();
        assert_eq!(updated, 2);
        assert_eq!(data.records[0].data[2], DType::Int(20));
        assert_eq!(data.records[1].data[2], DType::Int(60));
    }
}
