//! 类型系统：`TypeKind`（纯类型）+ `TypeInfo`（类型 + 元信息）+ 类型串解析器。
//!
//! # 核心设计
//!
//! - `TypeKind::Unresolved(String)` 承载"引用了尚未定义的类型"。类型串**语法合法即解析成功**，
//!   "引用不存在"是**语义错误**（依赖缺失），由编译阶段产生诊断 —— 这是增量编译"创建即校验、
//!   后续恢复"能够成立的关键。
//! - `TypeResolver` trait 让解析器与符号表解耦：解析时只查询"这个名字是 enum 还是 bean"，
//!   不直接依赖符号表结构，便于单元测试注入假解析器。

use std::collections::HashMap;

// ============================================================================
// 类型引用分类（解析器查询结果）
// ============================================================================

/// 一个类型名在符号表中的分类。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypeRef {
    Enum,
    Bean,
    Record,
}

/// 类型解析器：把 full_name 解析为 enum / bean，或 `None`（未定义）。
pub trait TypeResolver {
    fn resolve(&self, full_name: &str) -> Option<TypeRef>;

    /// Bean 的层级字段名（含父类，从根到自身）。用于 shadow 冲突与表索引列校验。
    fn bean_field_names(&self, full_name: &str) -> Option<Vec<String>> {
        let _ = full_name;
        None
    }

    /// Bean 的层级字段（含父类，从根到自身），完整字段名 + 类型。
    /// 用于数据加载（解码 JSON 记录时需要知道每个字段的类型）。
    fn bean_hierarchy_fields(&self, full_name: &str) -> Option<Vec<(String, TypeInfo)>> {
        let _ = full_name;
        None
    }

    /// Record 的索引定义（无继承，字段自身声明）。
    fn record_indexes(&self, full_name: &str) -> Option<Vec<crate::defs::TableIndex>> {
        let _ = full_name;
        None
    }
}

/// 空解析器：所有类型名都视为未解析（纯语法测试用）。
pub struct EmptyResolver;

impl TypeResolver for EmptyResolver {
    fn resolve(&self, _full_name: &str) -> Option<TypeRef> {
        None
    }
}

/// 基于 HashMap 的解析器（测试用）。
pub struct MapResolver {
    pub enums: HashMap<String, ()>,
    pub beans: HashMap<String, ()>,
}

impl MapResolver {
    pub fn new() -> Self {
        Self {
            enums: HashMap::new(),
            beans: HashMap::new(),
        }
    }

    pub fn with_enum(mut self, name: &str) -> Self {
        self.enums.insert(name.to_string(), ());
        self
    }

    pub fn with_bean(mut self, name: &str) -> Self {
        self.beans.insert(name.to_string(), ());
        self
    }
}

impl Default for MapResolver {
    fn default() -> Self {
        Self::new()
    }
}

impl TypeResolver for MapResolver {
    fn resolve(&self, full_name: &str) -> Option<TypeRef> {
        if self.enums.contains_key(full_name) {
            Some(TypeRef::Enum)
        } else if self.beans.contains_key(full_name) {
            Some(TypeRef::Bean)
        } else {
            None
        }
    }
}

// ============================================================================
// TypeKind
// ============================================================================

/// 纯类型描述符。
#[derive(Debug, Clone, PartialEq, Default)]
pub enum TypeKind {
    // ── 基础类型 ──
    #[default]
    Bool,
    I8,
    I16,
    I32,
    I64,
    U8,
    U16,
    U32,
    U64,
    F32,
    F64,
    Str,
    Text,
    DateTime,

    // ── 容器类型 ──
    Array(Box<TypeInfo>),
    List(Box<TypeInfo>),
    Set(Box<TypeInfo>),
    /// key / value（key 合法性在编译阶段校验）
    Map(Box<TypeInfo>, Box<TypeInfo>),

    // ── 引用类型 ──
    /// 已解析的枚举引用（full_name）
    Enum(String),
    /// 已解析的 Bean 引用（full_name）
    Bean(String),
    /// 跨表引用（表 full_name）
    Ref(String),
    /// 未解析的类型名（依赖缺失，编译阶段诊断）
    Unresolved(String),
}

// ============================================================================
// TypeInfo
// ============================================================================

/// 完整类型：类型种类 + 可空性 + 标签。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TypeInfo {
    pub kind: TypeKind,
    pub nullable: bool,
    pub tags: HashMap<String, String>,
}

impl TypeInfo {
    pub fn new(kind: TypeKind) -> Self {
        Self {
            kind,
            nullable: false,
            tags: HashMap::new(),
        }
    }

    /// 可读的类型串（用于诊断与展示）。
    pub fn type_name(&self) -> String {
        match &self.kind {
            TypeKind::Bool => "bool".into(),
            TypeKind::I8 => "i8".into(),
            TypeKind::I16 => "i16".into(),
            TypeKind::I32 => "i32".into(),
            TypeKind::I64 => "i64".into(),
            TypeKind::U8 => "u8".into(),
            TypeKind::U16 => "u16".into(),
            TypeKind::U32 => "u32".into(),
            TypeKind::U64 => "u64".into(),
            TypeKind::F32 => "f32".into(),
            TypeKind::F64 => "f64".into(),
            TypeKind::Str => "string".into(),
            TypeKind::Text => "text".into(),
            TypeKind::DateTime => "datetime".into(),
            TypeKind::Array(t) => format!("array<{}>", t.type_name()),
            TypeKind::List(t) => format!("list<{}>", t.type_name()),
            TypeKind::Set(t) => format!("set<{}>", t.type_name()),
            TypeKind::Map(k, v) => format!("map<{},{}>", k.type_name(), v.type_name()),
            TypeKind::Enum(n) | TypeKind::Bean(n) | TypeKind::Ref(n) | TypeKind::Unresolved(n) => {
                n.clone()
            }
        }
    }

    pub fn is_container(&self) -> bool {
        matches!(
            self.kind,
            TypeKind::Array(_) | TypeKind::List(_) | TypeKind::Set(_) | TypeKind::Map(_, _)
        )
    }

    pub fn is_numeric(&self) -> bool {
        matches!(
            self.kind,
            TypeKind::Bool
                | TypeKind::I8
                | TypeKind::I16
                | TypeKind::I32
                | TypeKind::I64
                | TypeKind::U8
                | TypeKind::U16
                | TypeKind::U32
                | TypeKind::U64
                | TypeKind::F32
                | TypeKind::F64
        )
    }

    /// 收集该类型引用的所有自定义类型 full_name（含 `Unresolved`），用于构建依赖图。
    pub fn collect_refs(&self) -> Vec<String> {
        let mut v = Vec::new();
        self.collect_refs_into(&mut v);
        v
    }

    fn collect_refs_into(&self, out: &mut Vec<String>) {
        match &self.kind {
            TypeKind::Enum(n) | TypeKind::Bean(n) | TypeKind::Ref(n) | TypeKind::Unresolved(n) => {
                out.push(n.clone())
            }
            TypeKind::Array(t) | TypeKind::List(t) | TypeKind::Set(t) => t.collect_refs_into(out),
            TypeKind::Map(k, v) => {
                k.collect_refs_into(out);
                v.collect_refs_into(out);
            }
            _ => {}
        }
    }

    /// 收集所有未解析引用（用于产生"未解析类型"诊断）。
    pub fn unresolved_refs(&self) -> Vec<String> {
        let mut v = Vec::new();
        self.collect_unresolved(&mut v);
        v
    }

    fn collect_unresolved(&self, out: &mut Vec<String>) {
        match &self.kind {
            TypeKind::Unresolved(n) => out.push(n.clone()),
            TypeKind::Array(t) | TypeKind::List(t) | TypeKind::Set(t) => t.collect_unresolved(out),
            TypeKind::Map(k, v) => {
                k.collect_unresolved(out);
                v.collect_unresolved(out);
            }
            _ => {}
        }
    }
}

// ============================================================================
// 类型串解析器
// ============================================================================

/// 解析类型串（含可空 `?`、后缀标签 `T(k=v,k2=v2)`、容器、别名、引用）。
pub fn parse_type(s: &str, resolver: &dyn TypeResolver) -> Result<TypeInfo, String> {
    let s = s.trim();
    if s.is_empty() {
        return Err("类型表达式不能为空".to_string());
    }

    // 1. 提取后缀圆括号标签
    let (type_str, tags) = extract_trailing_paren_tags(s)?;

    // 2. 可空后缀 `?`
    let (type_str, nullable) = match type_str.strip_suffix('?') {
        Some(rest) => (rest.trim().to_string(), true),
        None => (type_str.trim().to_string(), false),
    };

    // 3. 解析类型表达式
    let kind = parse_type_expr(&type_str, resolver)?;

    Ok(TypeInfo {
        kind,
        nullable,
        tags,
    })
}

/// 提取 `T(k=v,...)` 后缀标签：返回 (类型串, 标签表)。
fn extract_trailing_paren_tags(s: &str) -> Result<(&str, HashMap<String, String>), String> {
    let s = s.trim();
    if !s.ends_with(')') {
        return Ok((s, HashMap::new()));
    }
    let bytes = s.as_bytes();
    let mut depth = 0usize;
    for i in (0..s.len()).rev() {
        match bytes[i] {
            b')' => depth += 1,
            b'(' => {
                depth -= 1;
                if depth == 0 {
                    let head = s[..i].trim();
                    let tail = &s[i + 1..s.len() - 1];
                    let tags = parse_tags(tail)?;
                    return Ok((head, tags));
                }
            }
            _ => {}
        }
    }
    Ok((s, HashMap::new()))
}

/// 解析 `k=v,k2=v2` 标签串（裸标签 `k` 等价于 `k=true`）。
fn parse_tags(s: &str) -> Result<HashMap<String, String>, String> {
    let mut tags = HashMap::new();
    let s = s.trim();
    if s.is_empty() {
        return Ok(tags);
    }
    for part in split_top_level(s, ',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let (k, v) = match part.find('=') {
            Some(eq) => (
                part[..eq].trim().to_string(),
                part[eq + 1..].trim().to_string(),
            ),
            None => (part.to_string(), "true".to_string()),
        };
        if k.is_empty() {
            return Err(format!("标签键不能为空: '{}'", s));
        }
        if tags.insert(k.clone(), v).is_some() {
            return Err(format!("重复的标签键: '{}'", k));
        }
    }
    Ok(tags)
}

/// 按顶层分隔符分裂（括号 / 方括号 / 尖括号内的分隔符不分裂）。
fn split_top_level(s: &str, sep: char) -> Vec<&str> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut last = 0usize;
    for (i, ch) in s.char_indices() {
        match ch {
            '(' | '[' | '{' | '<' => depth += 1,
            ')' | ']' | '}' | '>' => depth -= 1,
            c if c == sep && depth == 0 => {
                out.push(&s[last..i]);
                last = i + 1;
            }
            _ => {}
        }
    }
    out.push(&s[last..]);
    out
}

/// 解析类型表达式（容器 / 基础 / 引用）。
fn parse_type_expr(s: &str, resolver: &dyn TypeResolver) -> Result<TypeKind, String> {
    let s = s.trim();
    if s.is_empty() {
        return Err("类型表达式不能为空".to_string());
    }

    if let Some(inner) = try_generic(s, "list") {
        return Ok(TypeKind::List(Box::new(parse_type(inner, resolver)?)));
    }
    if let Some(inner) = try_generic(s, "set") {
        return Ok(TypeKind::Set(Box::new(parse_type(inner, resolver)?)));
    }
    if let Some(inner) = try_generic(s, "array") {
        return Ok(TypeKind::Array(Box::new(parse_type(inner, resolver)?)));
    }
    if let Some(inner) = try_generic(s, "map") {
        let params = split_top_level(inner, ',');
        if params.len() != 2 {
            return Err(format!("map 需要 2 个参数，得到 {} 个", params.len()));
        }
        let key = parse_type(params[0], resolver)?;
        let value = parse_type(params[1], resolver)?;
        return Ok(TypeKind::Map(Box::new(key), Box::new(value)));
    }
    if let Some(inner) = try_generic(s, "ref") {
        return Ok(TypeKind::Ref(inner.trim().to_string()));
    }

    let lower = s.to_lowercase();
    let primitive = match lower.as_str() {
        "bool" => TypeKind::Bool,
        "i8" | "int8" => TypeKind::I8,
        "u8" | "uint8" | "byte" => TypeKind::U8,
        "i16" | "int16" | "short" => TypeKind::I16,
        "u16" | "uint16" => TypeKind::U16,
        "i32" | "int32" | "int" => TypeKind::I32,
        "u32" | "uint32" => TypeKind::U32,
        "i64" | "int64" | "long" => TypeKind::I64,
        "u64" | "uint64" | "ulong" => TypeKind::U64,
        "f32" | "float" => TypeKind::F32,
        "f64" | "double" => TypeKind::F64,
        "string" => TypeKind::Str,
        "text" => TypeKind::Text,
        "datetime" | "time" => TypeKind::DateTime,
        _ => {
            // 引用类型：查 resolver 分类
            return Ok(match resolver.resolve(s) {
                Some(TypeRef::Enum) => TypeKind::Enum(s.to_string()),
                Some(TypeRef::Bean) => TypeKind::Bean(s.to_string()),
                Some(TypeRef::Record) => TypeKind::Bean(s.to_string()),
                None => TypeKind::Unresolved(s.to_string()),
            });
        }
    };
    Ok(primitive)
}

/// 尝试提取 `name<...>` 的泛型内部（忽略大小写、允许空格）。
fn try_generic<'a>(s: &'a str, name: &str) -> Option<&'a str> {
    let s = s.trim();
    if s.len() <= name.len() + 2 {
        return None;
    }
    if !s[..name.len()].eq_ignore_ascii_case(name) {
        return None;
    }
    let rest = s[name.len()..].trim_start();
    if !rest.starts_with('<') {
        return None;
    }
    find_matching_angle(rest)
}

/// 找最外层尖括号的匹配内容（`<...>` 内部）。
fn find_matching_angle(s: &str) -> Option<&str> {
    let s = s.trim_start();
    if !s.starts_with('<') {
        return None;
    }
    let mut depth = 0u32;
    for (i, ch) in s.char_indices() {
        match ch {
            '<' => depth += 1,
            '>' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&s[1..i]);
                }
            }
            _ => {}
        }
    }
    None
}

// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_primitive_and_alias() {
        let r = EmptyResolver;
        assert_eq!(parse_type("int", &r).unwrap().type_name(), "i32");
        assert_eq!(parse_type("long", &r).unwrap().type_name(), "i64");
        assert_eq!(parse_type("byte", &r).unwrap().type_name(), "u8");
        assert_eq!(parse_type("float", &r).unwrap().type_name(), "f32");
        assert_eq!(parse_type("double", &r).unwrap().type_name(), "f64");
        assert_eq!(parse_type("time", &r).unwrap().type_name(), "datetime");
        assert_eq!(parse_type("string", &r).unwrap().type_name(), "string");
    }

    #[test]
    fn parse_container_and_nested() {
        let r = EmptyResolver;
        assert_eq!(
            parse_type("list<int>", &r).unwrap().type_name(),
            "list<i32>"
        );
        assert_eq!(
            parse_type("list<list<int>>", &r).unwrap().type_name(),
            "list<list<i32>>"
        );
        assert_eq!(
            parse_type("map<string,int>", &r).unwrap().type_name(),
            "map<string,i32>"
        );
        assert_eq!(
            parse_type("list<map<string, list<int>>>", &r)
                .unwrap()
                .type_name(),
            "list<map<string,list<i32>>>"
        );
        // 空格容错
        assert_eq!(
            parse_type("map< string , int >", &r).unwrap().type_name(),
            "map<string,i32>"
        );
    }

    #[test]
    fn parse_nullable_and_tags() {
        let r = EmptyResolver;
        assert!(parse_type("int?", &r).unwrap().nullable);
        let ti = parse_type("int(range=[1,100])", &r).unwrap();
        assert_eq!(ti.tags.get("range").unwrap(), "[1,100]");
        // 容器上的标签
        let ti = parse_type("list<int>(range=[1,3])", &r).unwrap();
        assert_eq!(ti.tags.get("range").unwrap(), "[1,3]");
    }

    #[test]
    fn parse_bare_and_valued_tags() {
        let r = EmptyResolver;
        let ti = parse_type("int(nonneg)", &r).unwrap();
        assert_eq!(ti.tags.get("nonneg").unwrap(), "true");

        let ti = parse_type("int(nonneg,range=[0,9])", &r).unwrap();
        assert_eq!(ti.tags.get("nonneg").unwrap(), "true");
        assert_eq!(ti.tags.get("range").unwrap(), "[0,9]");

        let ti = parse_type("int(a,b=c)", &r).unwrap();
        assert_eq!(ti.tags.get("a").unwrap(), "true");
        assert_eq!(ti.tags.get("b").unwrap(), "c");
    }

    #[test]
    fn parse_ref_and_unresolved() {
        let r = MapResolver::new()
            .with_enum("Quality")
            .with_bean("game.ItemCfg");
        match parse_type("Quality", &r).unwrap().kind {
            TypeKind::Enum(n) => assert_eq!(n, "Quality"),
            _ => panic!("应为 Enum"),
        }
        match parse_type("game.ItemCfg", &r).unwrap().kind {
            TypeKind::Bean(n) => assert_eq!(n, "game.ItemCfg"),
            _ => panic!("应为 Bean"),
        }
        match parse_type("NotExist", &r).unwrap().kind {
            TypeKind::Unresolved(n) => assert_eq!(n, "NotExist"),
            _ => panic!("应为 Unresolved"),
        }
    }

    #[test]
    fn collect_refs_includes_unresolved() {
        let r = MapResolver::new().with_enum("Quality");
        let ti = parse_type("list<NotExist>", &r).unwrap();
        assert_eq!(ti.collect_refs(), vec!["NotExist".to_string()]);
        assert_eq!(ti.unresolved_refs(), vec!["NotExist".to_string()]);
    }

    #[test]
    fn parse_invalid_type_errs() {
        let r = EmptyResolver;
        assert!(parse_type("", &r).is_err());
        assert!(parse_type("map<int>", &r).is_err()); // map 需 2 参数
    }
}
