//! 定义层：Raw（磁盘 JSON 的哑数据）→ Def（编译后语义模型）+ 单定义编译。
//!
//! 单定义编译是增量编译的基石：每个编译函数返回
//! `(编译结果, 依赖列表, 诊断)`。依赖列表（它引用了哪些 full_name）由符号表用于构建依赖图；
//! "引用了不存在的类型"通过 `TypeKind::Unresolved` 表现为诊断而非解析失败。

use crate::diagnostic::Diagnostic;
use crate::types::{TypeInfo, TypeRef, TypeResolver, parse_type};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

// ============================================================================
// 定义种类
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DefKind {
    Enum,
    Bean,
    Table,
    Record,
}

impl DefKind {
    pub fn label(&self) -> &'static str {
        match self {
            DefKind::Enum => "枚举",
            DefKind::Bean => "结构",
            DefKind::Table => "表",
            DefKind::Record => "记录",
        }
    }
}

/// 构造 full_name：`module.name`，空 module 时为裸名。
pub fn full_name(module: &str, name: &str) -> String {
    if module.is_empty() {
        name.to_string()
    } else {
        format!("{}.{}", module, name)
    }
}

// ============================================================================
// Raw 层（serde，对齐前端 types.ts 的 JSON 格式）
// ============================================================================

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RawEnumItem {
    pub name: String,
    #[serde(default)]
    pub value: Option<String>,
    #[serde(default)]
    pub alias: Option<String>,
    #[serde(default)]
    pub comment: Option<String>,
    #[serde(default)]
    pub properties: HashMap<String, String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RawEnum {
    pub name: String,
    #[serde(default)]
    pub module: String,
    #[serde(default)]
    pub comment: Option<String>,
    #[serde(default)]
    pub alias: Option<String>,
    #[serde(default)]
    pub is_flag: bool,
    #[serde(default)]
    pub is_unique: bool,
    #[serde(default)]
    pub items: Vec<RawEnumItem>,
    #[serde(default)]
    pub groups: Vec<String>,
    #[serde(default)]
    pub properties: HashMap<String, String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RawField {
    pub name: String,
    #[serde(rename = "type")]
    pub r#type: String,
    #[serde(default)]
    pub comment: Option<String>,
    #[serde(default)]
    pub groups: Vec<String>,
    #[serde(default)]
    pub properties: HashMap<String, String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RawBean {
    pub name: String,
    #[serde(default)]
    pub module: String,
    #[serde(default)]
    pub comment: Option<String>,
    #[serde(default)]
    pub alias: Option<String>,
    #[serde(default)]
    pub sep: Option<String>,
    #[serde(default)]
    pub is_value: bool,
    #[serde(default)]
    pub parent: Option<String>,
    #[serde(default)]
    pub fields: Vec<RawField>,
    #[serde(default)]
    pub groups: Vec<String>,
    #[serde(default)]
    pub properties: HashMap<String, String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RawTable {
    pub name: String,
    #[serde(default)]
    pub module: String,
    #[serde(default)]
    pub comment: Option<String>,
    #[serde(default)]
    pub index: Option<String>,
    pub value_type: String,
    #[serde(default)]
    pub mode: Option<String>,
    #[serde(default)]
    pub input: Vec<String>,
    #[serde(default)]
    pub output: Option<String>,
    #[serde(default)]
    pub groups: Vec<String>,
    #[serde(default)]
    pub properties: HashMap<String, String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RawRecord {
    pub name: String,
    #[serde(default)]
    pub module: String,
    #[serde(default)]
    pub comment: Option<String>,
    #[serde(default)]
    pub fields: Vec<RawField>,
    #[serde(default)]
    pub groups: Vec<String>,
    #[serde(default)]
    pub properties: HashMap<String, String>,
}

/// 任意一种定义的 raw（程序内部统一传递）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RawDef {
    Enum(RawEnum),
    Bean(RawBean),
    Table(RawTable),
    Record(RawRecord),
}

impl RawDef {
    pub fn kind(&self) -> DefKind {
        match self {
            RawDef::Enum(_) => DefKind::Enum,
            RawDef::Bean(_) => DefKind::Bean,
            RawDef::Table(_) => DefKind::Table,
            RawDef::Record(_) => DefKind::Record,
        }
    }

    pub fn full_name(&self) -> String {
        match self {
            RawDef::Enum(r) => full_name(&r.module, &r.name),
            RawDef::Bean(r) => full_name(&r.module, &r.name),
            RawDef::Table(r) => full_name(&r.module, &r.name),
            RawDef::Record(r) => full_name(&r.module, &r.name),
        }
    }
}

// ============================================================================
// Def 层（编译后）
// ============================================================================

#[derive(Debug, Clone)]
pub struct DefEnumItem {
    pub name: String,
    pub alias: Option<String>,
    pub value: i64,
}

#[derive(Debug, Clone)]
pub struct DefEnum {
    pub name: String, // full_name
    pub module: String,
    pub comment: Option<String>,
    pub alias: Option<String>,
    pub is_flag: bool,
    pub items: Vec<DefEnumItem>,
    pub groups: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct DefField {
    pub name: String,
    pub type_str: String,
    pub type_info: TypeInfo,
    pub comment: Option<String>,
    pub groups: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct DefBean {
    pub name: String, // full_name
    pub module: String,
    pub comment: Option<String>,
    pub parent: Option<String>, // full_name
    pub fields: Vec<DefField>,  // 自己的字段
    /// 层级字段名（含父类，从根到自身）。用于 shadow 冲突与表索引列校验。
    pub hierarchy_field_names: Vec<String>,
    /// 层级字段（含父类，从根到自身），完整字段名 + 类型。用于数据加载。
    pub hierarchy_fields: Vec<DefField>,
    pub groups: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TableMode {
    One,
    Map,
    List,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableIndex {
    pub columns: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct DefTable {
    pub name: String, // full_name
    pub module: String,
    pub comment: Option<String>,
    pub mode: TableMode,
    pub index: Vec<TableIndex>,
    pub value_type: String,
    pub input: Vec<String>,
    pub groups: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct DefRecord {
    pub name: String, // full_name
    pub module: String,
    pub comment: Option<String>,
    pub fields: Vec<DefField>,
    pub groups: Vec<String>,
}

/// 编译后定义的统一载体。
#[derive(Debug, Clone)]
pub enum DefValue {
    Enum(DefEnum),
    Bean(DefBean),
    Table(DefTable),
    Record(DefRecord),
}

impl DefValue {
    pub fn kind(&self) -> DefKind {
        match self {
            DefValue::Enum(_) => DefKind::Enum,
            DefValue::Bean(_) => DefKind::Bean,
            DefValue::Table(_) => DefKind::Table,
            DefValue::Record(_) => DefKind::Record,
        }
    }
}

// ============================================================================
// 单定义编译
// ============================================================================

/// 编译一个枚举。无类型依赖。
pub fn compile_enum(raw: &RawEnum) -> (DefEnum, Vec<String>, Vec<Diagnostic>) {
    let full = full_name(&raw.module, &raw.name);
    let mut diags = Vec::new();

    if raw.name.is_empty() {
        diags.push(Diagnostic::error(&full, "枚举 name 不能为空"));
    }

    let mut items = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    // 自动递增：非 flag 从 0 开始 +1；flag 从 1 开始位左移
    let mut next: i64 = if raw.is_flag { 1 } else { 0 };

    for it in &raw.items {
        if !seen.insert(it.name.clone()) {
            diags.push(Diagnostic::error(
                &full,
                format!("枚举项 '{}' 重复", it.name),
            ));
        }

        let mut value = next;
        if let Some(v) = &it.value {
            match parse_int_literal(v) {
                Ok(val) => value = val,
                Err(_) => {
                    // 名称引用（引用前面已定义的枚举项）
                    if let Some(prev) = items.iter().find(|p: &&DefEnumItem| p.name == *v) {
                        value = prev.value;
                    } else {
                        diags.push(Diagnostic::error(
                            &full,
                            format!("枚举项 '{}' 的值 '{}' 无法解析", it.name, v),
                        ));
                    }
                }
            }
        }
        // 推进 next（无论显式还是自动）
        next = if raw.is_flag { value << 1 } else { value + 1 };

        items.push(DefEnumItem {
            name: it.name.clone(),
            alias: it.alias.clone(),
            value,
        });
    }

    // PostCompile：i32 值域检查
    for it in &items {
        if it.value < i32::MIN as i64 || it.value > i32::MAX as i64 {
            diags.push(Diagnostic::error(
                &full,
                format!("枚举项 '{}' 的值 {} 超出 i32 范围", it.name, it.value),
            ));
        }
    }

    let def = DefEnum {
        name: full,
        module: raw.module.clone(),
        comment: raw.comment.clone(),
        alias: raw.alias.clone(),
        is_flag: raw.is_flag,
        items,
        groups: raw.groups.clone(),
    };
    (def, Vec::new(), diags)
}

/// 编译一个 Bean（含父类解析、字段类型解析、shadow 检查）。
pub fn compile_bean(
    raw: &RawBean,
    resolver: &dyn TypeResolver,
) -> (DefBean, Vec<String>, Vec<Diagnostic>) {
    let full = full_name(&raw.module, &raw.name);
    let mut diags = Vec::new();
    let mut deps: Vec<String> = Vec::new();

    if raw.name.is_empty() {
        diags.push(Diagnostic::error(&full, "Bean name 不能为空"));
    }

    // 1. 父类解析
    let mut parent: Option<String> = None;
    if let Some(p) = raw
        .parent
        .as_deref()
        .map(str::trim)
        .filter(|p| !p.is_empty())
    {
        match resolver.resolve(p) {
            Some(TypeRef::Bean) => {
                deps.push(p.to_string());
                parent = Some(p.to_string());
            }
            Some(TypeRef::Enum) => diags.push(Diagnostic::error(
                &full,
                format!("父类 '{}' 是枚举，不能作为 Bean 的父类", p),
            )),
            None => {
                deps.push(p.to_string());
                diags.push(Diagnostic::error(
                    &full,
                    format!("引用了不存在的父类 '{}'", p),
                ));
            }
        }
    }

    // 2. 字段编译
    let mut fields = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for rf in &raw.fields {
        if !seen.insert(rf.name.clone()) {
            diags.push(Diagnostic::error(&full, format!("字段 '{}' 重复", rf.name)));
        }
        match parse_type(&rf.r#type, resolver) {
            Ok(ti) => {
                for r in ti.collect_refs() {
                    deps.push(r);
                }
                for u in ti.unresolved_refs() {
                    diags.push(Diagnostic::error(
                        &full,
                        format!("字段 '{}' 引用了未定义的类型 '{}'", rf.name, u),
                    ));
                }
                fields.push(DefField {
                    name: rf.name.clone(),
                    type_str: rf.r#type.clone(),
                    type_info: ti,
                    comment: rf.comment.clone(),
                    groups: rf.groups.clone(),
                });
            }
            Err(e) => diags.push(Diagnostic::error(
                &full,
                format!("字段 '{}' 类型 '{}' 无效: {}", rf.name, rf.r#type, e),
            )),
        }
    }

    // 3. 层级字段（含父类，完整类型）+ shadow 检查
    let mut hierarchy_field_names: Vec<String> = Vec::new();
    let mut hierarchy_fields: Vec<DefField> = Vec::new();
    if let Some(p) = parent.as_ref()
        && let Some(pfields) = resolver.bean_hierarchy_fields(p)
    {
        let own: HashSet<&str> = raw.fields.iter().map(|f| f.name.as_str()).collect();
        for (pn, pti) in &pfields {
            if own.contains(pn.as_str()) {
                diags.push(Diagnostic::error(
                    &full,
                    format!("字段 '{}' 与父类 '{}' 的字段冲突", pn, p),
                ));
            }
            hierarchy_field_names.push(pn.clone());
            hierarchy_fields.push(DefField {
                name: pn.clone(),
                type_str: pti.type_name(),
                type_info: pti.clone(),
                comment: None,
                groups: Vec::new(),
            });
        }
    }
    hierarchy_field_names.extend(raw.fields.iter().map(|f| f.name.clone()));
    hierarchy_fields.extend(fields.clone());

    let def = DefBean {
        name: full,
        module: raw.module.clone(),
        comment: raw.comment.clone(),
        parent,
        fields,
        hierarchy_field_names,
        hierarchy_fields,
        groups: raw.groups.clone(),
    };
    (def, deps, diags)
}

/// 编译一个表（mode / value_type / 索引列校验）。
pub fn compile_table(
    raw: &RawTable,
    resolver: &dyn TypeResolver,
) -> (DefTable, Vec<String>, Vec<Diagnostic>) {
    let full = full_name(&raw.module, &raw.name);
    let mut diags = Vec::new();
    let mut deps: Vec<String> = Vec::new();

    if raw.name.is_empty() {
        diags.push(Diagnostic::error(&full, "表 name 不能为空"));
    }

    // mode
    let mode = match raw.mode.as_deref() {
        None | Some("") | Some("one") => TableMode::One,
        Some("map") => TableMode::Map,
        Some("list") => TableMode::List,
        Some(m) => {
            diags.push(Diagnostic::error(&full, format!("未知表模式 '{}'", m)));
            TableMode::One
        }
    };

    // value_type
    if raw.value_type.trim().is_empty() {
        diags.push(Diagnostic::error(&full, "value_type 不能为空"));
    } else {
        match resolver.resolve(raw.value_type.trim()) {
            Some(TypeRef::Bean) => deps.push(raw.value_type.trim().to_string()),
            Some(TypeRef::Enum) => diags.push(Diagnostic::error(
                &full,
                format!("value_type '{}' 是枚举，不能作为表记录类型", raw.value_type),
            )),
            None => {
                deps.push(raw.value_type.trim().to_string());
                diags.push(Diagnostic::error(
                    &full,
                    format!("引用了不存在的 value_type '{}'", raw.value_type),
                ));
            }
        }
    }

    // 索引解析
    let mut index = parse_index(raw.index.as_deref(), &full, &mut diags);

    // map 模式空索引 → 取 value_type 第一个字段
    let first_field = resolver
        .bean_field_names(raw.value_type.trim())
        .and_then(|f| f.first().cloned());
    if mode == TableMode::Map
        && index.is_empty()
        && let Some(f) = &first_field
    {
        index.push(TableIndex {
            columns: vec![f.clone()],
        });
    }

    // 索引列校验
    if let Some(fnames) = resolver.bean_field_names(raw.value_type.trim()) {
        for idx in &index {
            for col in &idx.columns {
                if !fnames.contains(col) {
                    diags.push(Diagnostic::error(
                        &full,
                        format!("索引列 '{}' 不存在于 '{}' 的字段中", col, raw.value_type),
                    ));
                }
            }
        }
    }

    let def = DefTable {
        name: full,
        module: raw.module.clone(),
        comment: raw.comment.clone(),
        mode,
        index,
        value_type: raw.value_type.clone(),
        input: raw.input.clone(),
        groups: raw.groups.clone(),
    };
    (def, deps, diags)
}

/// 编译一个 Record（无继承的 Bean）。
pub fn compile_record(
    raw: &RawRecord,
    resolver: &dyn TypeResolver,
) -> (DefRecord, Vec<String>, Vec<Diagnostic>) {
    let full = full_name(&raw.module, &raw.name);
    let mut diags = Vec::new();
    let mut deps: Vec<String> = Vec::new();

    if raw.name.is_empty() {
        diags.push(Diagnostic::error(&full, "Record name 不能为空"));
    }

    let mut fields = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for rf in &raw.fields {
        if !seen.insert(rf.name.clone()) {
            diags.push(Diagnostic::error(&full, format!("字段 '{}' 重复", rf.name)));
        }
        match parse_type(&rf.r#type, resolver) {
            Ok(ti) => {
                for r in ti.collect_refs() {
                    deps.push(r);
                }
                for u in ti.unresolved_refs() {
                    diags.push(Diagnostic::error(
                        &full,
                        format!("字段 '{}' 引用了未定义的类型 '{}'", rf.name, u),
                    ));
                }
                fields.push(DefField {
                    name: rf.name.clone(),
                    type_str: rf.r#type.clone(),
                    type_info: ti,
                    comment: rf.comment.clone(),
                    groups: rf.groups.clone(),
                });
            }
            Err(e) => diags.push(Diagnostic::error(
                &full,
                format!("字段 '{}' 类型 '{}' 无效: {}", rf.name, rf.r#type, e),
            )),
        }
    }

    let def = DefRecord {
        name: full,
        module: raw.module.clone(),
        comment: raw.comment.clone(),
        fields,
        groups: raw.groups.clone(),
    };
    (def, deps, diags)
}

// ============================================================================
// 辅助
// ============================================================================

/// 解析整数字面量（十进制 / 0x 十六进制 / 0b 二进制 / `_` 分隔符 / 负号）。
pub fn parse_int_literal(s: &str) -> Result<i64, String> {
    let s = s.trim().replace('_', "");
    if s.is_empty() {
        return Err("空值".to_string());
    }
    let (neg, body) = match s.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, s.as_str()),
    };
    let val = if let Some(hex) = body.strip_prefix("0x").or_else(|| body.strip_prefix("0X")) {
        i64::from_str_radix(hex, 16).map_err(|_| format!("非法十六进制 '{}'", s))?
    } else if let Some(bin) = body.strip_prefix("0b").or_else(|| body.strip_prefix("0B")) {
        i64::from_str_radix(bin, 2).map_err(|_| format!("非法二进制 '{}'", s))?
    } else {
        body.parse::<i64>()
            .map_err(|_| format!("非法整数 '{}'", s))?
    };
    Ok(if neg { -val } else { val })
}

/// 解析索引串：`a+b` 联合、`a,b` 多键。
pub fn parse_index(s: Option<&str>, full: &str, diags: &mut Vec<Diagnostic>) -> Vec<TableIndex> {
    let mut out = Vec::new();
    let Some(s) = s else { return out };
    let s = s.trim();
    if s.is_empty() {
        return out;
    }
    for seg in s.split(',') {
        let seg = seg.trim();
        if seg.is_empty() {
            continue;
        }
        let cols: Vec<String> = seg
            .split('+')
            .map(str::trim)
            .filter(|c| !c.is_empty())
            .map(str::to_string)
            .collect();
        if cols.is_empty() {
            diags.push(Diagnostic::error(full, format!("非法索引段 '{}'", seg)));
            continue;
        }
        out.push(TableIndex { columns: cols });
    }
    out
}

// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::MapResolver;

    #[test]
    fn parse_int_various() {
        assert_eq!(parse_int_literal("0").unwrap(), 0);
        assert_eq!(parse_int_literal("42").unwrap(), 42);
        assert_eq!(parse_int_literal("-7").unwrap(), -7);
        assert_eq!(parse_int_literal("0xff").unwrap(), 255);
        assert_eq!(parse_int_literal("0b1010").unwrap(), 10);
        assert_eq!(parse_int_literal("123_456").unwrap(), 123456);
        assert!(parse_int_literal("abc").is_err());
    }

    #[test]
    fn enum_auto_increment() {
        let raw = RawEnum {
            name: "Quality".into(),
            items: vec![
                RawEnumItem {
                    name: "White".into(),
                    ..Default::default()
                },
                RawEnumItem {
                    name: "Green".into(),
                    ..Default::default()
                },
                RawEnumItem {
                    name: "Blue".into(),
                    value: Some("10".into()),
                    ..Default::default()
                },
                RawEnumItem {
                    name: "Purple".into(),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let (def, _, diags) = compile_enum(&raw);
        assert!(diags.is_empty(), "不应有诊断: {:?}", diags);
        assert_eq!(def.items[0].value, 0);
        assert_eq!(def.items[1].value, 1);
        assert_eq!(def.items[2].value, 10);
        assert_eq!(def.items[3].value, 11);
    }

    #[test]
    fn enum_flag_increment() {
        let raw = RawEnum {
            name: "Flags".into(),
            is_flag: true,
            items: vec![
                RawEnumItem {
                    name: "A".into(),
                    ..Default::default()
                },
                RawEnumItem {
                    name: "B".into(),
                    ..Default::default()
                },
                RawEnumItem {
                    name: "C".into(),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let (def, _, diags) = compile_enum(&raw);
        assert!(diags.is_empty());
        assert_eq!(def.items[0].value, 1);
        assert_eq!(def.items[1].value, 2);
        assert_eq!(def.items[2].value, 4);
    }

    #[test]
    fn enum_dup_item_diagnosed() {
        let raw = RawEnum {
            name: "E".into(),
            items: vec![
                RawEnumItem {
                    name: "A".into(),
                    ..Default::default()
                },
                RawEnumItem {
                    name: "A".into(),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let (_, _, diags) = compile_enum(&raw);
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("重复"));
    }

    #[test]
    fn bean_field_shadow_diagnosed() {
        // 父类 Base 有字段 id；子类也声明 id → 冲突。用自定义 resolver 模拟父类字段。
        struct R;
        impl TypeResolver for R {
            fn resolve(&self, n: &str) -> Option<TypeRef> {
                if n == "Base" {
                    Some(TypeRef::Bean)
                } else {
                    None
                }
            }
            fn bean_field_names(&self, n: &str) -> Option<Vec<String>> {
                if n == "Base" {
                    Some(vec!["id".into(), "name".into()])
                } else {
                    None
                }
            }
            fn bean_hierarchy_fields(&self, n: &str) -> Option<Vec<(String, TypeInfo)>> {
                if n == "Base" {
                    Some(vec![
                        ("id".to_string(), TypeInfo::new(crate::types::TypeKind::I32)),
                        (
                            "name".to_string(),
                            TypeInfo::new(crate::types::TypeKind::Str),
                        ),
                    ])
                } else {
                    None
                }
            }
        }
        let raw = RawBean {
            name: "Child".into(),
            parent: Some("Base".into()),
            fields: vec![RawField {
                name: "id".into(),
                r#type: "int".into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        let (_, _, diags) = compile_bean(&raw, &R);
        assert!(diags.iter().any(|d| d.message.contains("冲突")));
    }

    #[test]
    fn bean_unresolved_field_diagnosed() {
        let raw = RawBean {
            name: "Item".into(),
            fields: vec![RawField {
                name: "q".into(),
                r#type: "Quality".into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        let (_, deps, diags) = compile_bean(&raw, &MapResolver::new());
        assert_eq!(deps, vec!["Quality".to_string()]);
        assert!(
            diags
                .iter()
                .any(|d| d.message.contains("未定义的类型 'Quality'"))
        );
    }

    #[test]
    fn table_index_parsing() {
        let mut diags = Vec::new();
        let idx = parse_index(Some("id"), "T", &mut diags);
        assert_eq!(idx.len(), 1);
        assert_eq!(idx[0].columns, vec!["id"]);

        let idx = parse_index(Some("a+b"), "T", &mut diags);
        assert_eq!(idx[0].columns, vec!["a", "b"]);

        let idx = parse_index(Some("a,b"), "T", &mut diags);
        assert_eq!(idx.len(), 2);
        assert_eq!(idx[0].columns, vec!["a"]);
        assert_eq!(idx[1].columns, vec!["b"]);
    }
}
