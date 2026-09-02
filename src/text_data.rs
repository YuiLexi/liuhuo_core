//! 自定义文本数据格式（简化 jsonl）：紧凑、一行一记录、git 友好。
//!
//! # 文件格式
//! ```text
//! #version:0.1
//! #record:game.ItemCfg
//! ---
//! :1|"药水"|Green|100
//! :2|"铁剑"|White|500@tag(dev,release)
//! ```
//!
//! - 字段顺序由 schema（bean 层级字段）决定，字段名不重复出现在每行
//! - 值语法（类型指导）：数字含 `0x`/`0b`/`_`、字符串 `"..."`、list `[a,b]`、set `(a,b)`、
//!   map `{k:v}`、bean `{v1|v2}`（`|` 分隔字段）
//! - 行尾 `@tag(a,b)` 行标签

use crate::data::IDataLoader;
use crate::defs::DefTable;
use crate::types::{TypeInfo, TypeKind};
use crate::value::{DType, DataContext, Record, TableData};
use std::collections::HashMap;
use std::path::Path;

// ============================================================================
// 序列化
// ============================================================================

/// 值 → 文本。
pub fn serialize_value(v: &DType) -> String {
    match v {
        DType::Null => "null".to_string(),
        DType::Bool(b) => b.to_string(),
        DType::Int(i) => i.to_string(),
        DType::UInt(u) => u.to_string(),
        DType::Float(f) => f.to_string(),
        DType::DateTime(d) => d.to_string(),
        DType::Str(s) | DType::Text(s) => quote(s),
        DType::Enum(_, val) => val.to_string(),
        DType::Bean(_, vals) => format!(
            "{{{}}}",
            vals.iter()
                .map(serialize_value)
                .collect::<Vec<_>>()
                .join("|")
        ),
        DType::List(vals) | DType::Array(vals) => format!(
            "[{}]",
            vals.iter()
                .map(serialize_value)
                .collect::<Vec<_>>()
                .join(",")
        ),
        DType::Set(vals) => format!(
            "({})",
            vals.iter()
                .map(serialize_value)
                .collect::<Vec<_>>()
                .join(",")
        ),
        DType::Map(entries) => format!(
            "{{{}}}",
            entries
                .iter()
                .map(|(k, v)| format!("{}:{}", serialize_value(k), serialize_value(v)))
                .collect::<Vec<_>>()
                .join(",")
        ),
    }
}

/// 整张表 → 文本（含头部元信息）。
pub fn table_to_text(table: &DefTable, data: &TableData) -> String {
    let mut out = String::new();
    out.push_str("#version:0.1\n");
    out.push_str(&format!("#record:{}\n", table.value_type));
    out.push_str("---\n");
    for record in &data.records {
        out.push(':');
        out.push_str(
            &record
                .data
                .iter()
                .map(serialize_value)
                .collect::<Vec<_>>()
                .join("|"),
        );
        if !record.tags.is_empty() {
            let tags: Vec<String> = record
                .tags
                .iter()
                .map(|(k, v)| format!("{}({})", k, v))
                .collect();
            out.push('@');
            out.push_str(&tags.join(","));
        }
        out.push('\n');
    }
    out
}

fn quote(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn unquote(s: &str) -> Result<String, String> {
    let s = s.trim();
    if !s.starts_with('"') || !s.ends_with('"') {
        return Err(format!("字符串必须以双引号包裹: '{}'", s));
    }
    let inner = &s[1..s.len() - 1];
    let mut out = String::new();
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('"') => out.push('"'),
                Some('\\') => out.push('\\'),
                Some('n') => out.push('\n'),
                Some(other) => {
                    out.push('\\');
                    out.push(other);
                }
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    Ok(out)
}

// ============================================================================
// 解析（类型指导）
// ============================================================================

/// 字符串值 + 类型 → DType（递归下降，类型指导）。
pub fn parse_value(s: &str, ti: &TypeInfo, ctx: &dyn DataContext) -> Result<DType, String> {
    let s = s.trim();
    match &ti.kind {
        TypeKind::Bool => match s {
            "true" => Ok(DType::Bool(true)),
            "false" => Ok(DType::Bool(false)),
            _ => Err(format!("非法 bool '{}'", s)),
        },
        TypeKind::I8 | TypeKind::I16 | TypeKind::I32 | TypeKind::I64 => {
            crate::defs::parse_int_literal(s).map(DType::Int)
        }
        TypeKind::U8 | TypeKind::U16 | TypeKind::U32 | TypeKind::U64 => {
            crate::defs::parse_int_literal(s).map(|v| DType::UInt(v as u64))
        }
        TypeKind::F32 | TypeKind::F64 => s
            .parse::<f64>()
            .map(DType::Float)
            .map_err(|_| format!("非法浮点 '{}'", s)),
        TypeKind::Str => unquote(s).map(DType::Str),
        TypeKind::Text => unquote(s).map(DType::Text),
        TypeKind::DateTime => s
            .parse::<i64>()
            .map(DType::DateTime)
            .map_err(|_| format!("非法时间戳 '{}'", s)),
        TypeKind::Enum(name) => {
            // 名字 / 别名 / 数值
            let v = ctx
                .enum_value(name, s)
                .or_else(|| crate::defs::parse_int_literal(s).ok())
                .ok_or_else(|| format!("枚举 '{}' 不包含 '{}'", name, s))?;
            Ok(DType::Enum(name.clone(), v))
        }
        TypeKind::Bean(name) => {
            let inner = strip_braces(s, '{', '}')?;
            let fields = ctx
                .bean_hierarchy_fields(name)
                .ok_or_else(|| format!("未知 Bean '{}'", name))?;
            let parts = split_top_level(inner, '|');
            if parts.len() != fields.len() {
                return Err(format!(
                    "Bean '{}' 需 {} 个字段，实际 {} 个",
                    name,
                    fields.len(),
                    parts.len()
                ));
            }
            let vals = parts
                .iter()
                .zip(&fields)
                .map(|(p, (_, fti))| parse_value(p, fti, ctx))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(DType::Bean(name.clone(), vals))
        }
        TypeKind::Array(elem) | TypeKind::List(elem) => {
            let inner = strip_braces(s, '[', ']')?;
            let vals = if inner.is_empty() {
                Vec::new()
            } else {
                split_top_level(inner, ',')
                    .iter()
                    .map(|p| parse_value(p, elem, ctx))
                    .collect::<Result<Vec<_>, _>>()?
            };
            Ok(match &ti.kind {
                TypeKind::Array(_) => DType::Array(vals),
                _ => DType::List(vals),
            })
        }
        TypeKind::Set(elem) => {
            let inner = strip_braces(s, '(', ')')?;
            let vals = if inner.is_empty() {
                Vec::new()
            } else {
                split_top_level(inner, ',')
                    .iter()
                    .map(|p| parse_value(p, elem, ctx))
                    .collect::<Result<Vec<_>, _>>()?
            };
            Ok(DType::Set(vals))
        }
        TypeKind::Map(k_ti, v_ti) => {
            let inner = strip_braces(s, '{', '}')?;
            let entries = if inner.is_empty() {
                Vec::new()
            } else {
                split_top_level(inner, ',')
                    .iter()
                    .map(|pair| {
                        let (k, v) = split_first_top_level(pair, ':');
                        if k.is_empty() || v.is_empty() {
                            return Err(format!("Map 条目需 k:v 格式: '{}'", pair));
                        }
                        // 字符串键允许裸形式（.lhd 的 {k=v} 语法）：自动补引号
                        let k_owned;
                        let k_ref = k;
                        let k = if matches!(k_ti.kind, TypeKind::Str | TypeKind::Text)
                            && !k_ref.trim_start().starts_with('"')
                        {
                            k_owned = format!("\"{}\"", k_ref.trim());
                            k_owned.as_str()
                        } else {
                            k_ref
                        };
                        Ok((parse_value(k, k_ti, ctx)?, parse_value(v, v_ti, ctx)?))
                    })
                    .collect::<Result<Vec<_>, String>>()?
            };
            Ok(DType::Map(entries))
        }
        TypeKind::Ref(_) => crate::defs::parse_int_literal(s).map(DType::Int),
        TypeKind::Unresolved(name) => Err(format!("类型 '{}' 未解析", name)),
    }
}

fn strip_braces(s: &str, open: char, close: char) -> Result<&str, String> {
    let s = s.trim();
    if !s.starts_with(open) || !s.ends_with(close) {
        return Err(format!("期望 {}...{} 包裹: '{}'", open, close, s));
    }
    Ok(&s[1..s.len() - 1])
}

/// 按顶层分隔符分裂（括号内不分裂）。
fn split_top_level(s: &str, sep: char) -> Vec<&str> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut last = 0usize;
    for (i, ch) in s.char_indices() {
        match ch {
            '(' | '[' | '{' | '<' => depth += 1,
            ')' | ']' | '}' | '>' => depth -= 1,
            c if c == sep && depth == 0 => {
                out.push(s[last..i].trim());
                last = i + 1;
            }
            _ => {}
        }
    }
    out.push(s[last..].trim());
    out
}

/// 按第一个顶层分隔符分裂（用于 map 的 k:v，v 可能含嵌套冒号）。
fn split_first_top_level(s: &str, sep: char) -> (&str, &str) {
    let mut depth = 0i32;
    for (i, ch) in s.char_indices() {
        match ch {
            '(' | '[' | '{' | '<' => depth += 1,
            ')' | ']' | '}' | '>' => depth -= 1,
            c if c == sep && depth == 0 => return (s[..i].trim(), s[i + 1..].trim()),
            _ => {}
        }
    }
    (s.trim(), "")
}

// ============================================================================
// TextDataLoader
// ============================================================================

/// 自定义文本数据加载器（.txt / .liuhuo）。
#[derive(Debug, Default)]
pub struct TextDataLoader;

impl IDataLoader for TextDataLoader {
    fn name(&self) -> &str {
        "text"
    }

    fn extensions(&self) -> &[&str] {
        &["txt", "liuhuo"]
    }

    fn load_table(
        &self,
        path: &Path,
        table: &DefTable,
        ctx: &dyn DataContext,
    ) -> Result<TableData, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("无法读取数据文件 '{}': {}", path.display(), e))?;
        load_from_str(&text, table, ctx)
    }
}

/// 从文本解析表数据。
pub fn load_from_str(
    text: &str,
    table: &DefTable,
    ctx: &dyn DataContext,
) -> Result<TableData, String> {
    let fields = ctx
        .bean_hierarchy_fields(&table.value_type)
        .ok_or_else(|| format!("未知 Bean '{}'", table.value_type))?;

    let mut data = TableData::new();
    for (line_no, raw_line) in text.lines().enumerate() {
        let line = raw_line.trim();
        // 跳过空行、注释、分隔符
        if line.is_empty() || line.starts_with('#') || line.starts_with("---") {
            continue;
        }
        // 去掉行首冒号
        let body = line.strip_prefix(':').unwrap_or(line);
        // 提取 @tag(...)
        let (body, tags) = extract_tags(body);
        // 按 | 分裂字段
        let parts = split_top_level(body, '|');
        if parts.len() != fields.len() {
            return Err(format!(
                "表 '{}' 第 {} 行需 {} 个字段，实际 {} 个: '{}'",
                table.name,
                line_no + 1,
                fields.len(),
                parts.len(),
                line
            ));
        }
        let vals = parts
            .iter()
            .zip(&fields)
            .map(|(p, (_, ti))| parse_value(p, ti, ctx))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("表 '{}' 第 {} 行: {}", table.name, line_no + 1, e))?;

        let mut record = Record::with_capacity(fields.len());
        record.bean = Some(table.value_type.clone());
        record.data = vals;
        record.tags = tags;
        data.push(record);
    }
    Ok(data)
}

/// 提取行尾 `@tag(a,b)` 标签。
fn extract_tags(body: &str) -> (&str, HashMap<String, String>) {
    let mut tags = HashMap::new();
    if let Some(at) = body.rfind('@') {
        let tag_section = &body[at + 1..];
        // tag(dev,release) 或 tag(dev)
        let mut rest = tag_section;
        while !rest.is_empty() {
            let Some(open) = rest.find('(') else { break };
            let Some(close) = rest.find(')') else { break };
            let key = rest[..open].trim();
            let value = rest[open + 1..close].trim();
            if !key.is_empty() {
                tags.insert(key.to_string(), value.to_string());
            }
            rest = &rest[close + 1..];
        }
        return (body[..at].trim(), tags);
    }
    (body.trim(), tags)
}

// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::TypeKind;

    #[test]
    fn serialize_and_parse_scalars() {
        let cases = [
            (DType::Int(42), "42"),
            (DType::Bool(true), "true"),
            (DType::Float(1.5), "1.5"),
            (DType::Str("hi".into()), "\"hi\""),
        ];
        for (v, expected) in cases {
            assert_eq!(serialize_value(&v), expected);
        }
    }

    #[test]
    fn serialize_containers() {
        assert_eq!(
            serialize_value(&DType::List(vec![DType::Int(1), DType::Int(2)])),
            "[1,2]"
        );
        assert_eq!(
            serialize_value(&DType::Set(vec![DType::Int(1), DType::Int(2)])),
            "(1,2)"
        );
        assert_eq!(
            serialize_value(&DType::Map(vec![(DType::Str("a".into()), DType::Int(1))])),
            "{\"a\":1}"
        );
    }

    #[test]
    fn parse_value_type_guided() {
        let ctx = EmptyCtx;
        let ti = TypeInfo::new(TypeKind::I32);
        assert_eq!(parse_value("42", &ti, &ctx).unwrap(), DType::Int(42));
        assert_eq!(parse_value("0xff", &ti, &ctx).unwrap(), DType::Int(255));

        let list_ti = TypeInfo::new(TypeKind::List(Box::new(TypeInfo::new(TypeKind::I32))));
        assert_eq!(
            parse_value("[1,2,3]", &list_ti, &ctx).unwrap(),
            DType::List(vec![DType::Int(1), DType::Int(2), DType::Int(3)])
        );

        let str_ti = TypeInfo::new(TypeKind::Str);
        assert_eq!(
            parse_value("\"你好\"", &str_ti, &ctx).unwrap(),
            DType::Str("你好".into())
        );
    }

    #[test]
    fn roundtrip_list_of_strings() {
        let ctx = EmptyCtx;
        let ti = TypeInfo::new(TypeKind::List(Box::new(TypeInfo::new(TypeKind::Str))));
        let v = DType::List(vec![DType::Str("a".into()), DType::Str("b|c".into())]);
        let text = serialize_value(&v);
        let back = parse_value(&text, &ti, &ctx).unwrap();
        assert_eq!(back, v);
    }

    struct EmptyCtx;
    impl DataContext for EmptyCtx {
        fn enum_value(&self, _: &str, _: &str) -> Option<i64> {
            None
        }
        fn bean_fields(&self, _: &str) -> Option<Vec<String>> {
            None
        }
    }
}
