//! 数据校验：字段级（range 等）+ 表级（unique key / single record）。
//!
//! trait + 注册表扩展点。诊断定位格式：`表名[行N].字段`。

use crate::defs::{DefTable, TableMode};
use crate::diagnostic::Diagnostic;
use crate::types::{TypeInfo, TypeKind};
use crate::value::{DType, TableData};
use std::collections::HashSet;

/// 字段级校验器（对单个字段值校验）。
pub trait IDataValidator: std::fmt::Debug + Send + Sync {
    fn name(&self) -> &str;
    fn validate(&self, value: &DType, type_info: &TypeInfo) -> Result<(), String>;
}

/// 表级校验器（对整表校验）。
pub trait ITableValidator: std::fmt::Debug + Send + Sync {
    fn name(&self) -> &str;
    fn validate(
        &self,
        table: &DefTable,
        data: &TableData,
        fields: &[(String, TypeInfo)],
    ) -> Vec<Diagnostic>;
}

/// 校验器注册表。
#[derive(Debug, Default)]
pub struct ValidatorRegistry {
    pub field_validators: Vec<Box<dyn IDataValidator>>,
    pub table_validators: Vec<Box<dyn ITableValidator>>,
}

impl ValidatorRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// 注册内置校验器（range / nonneg / unique-key / single-record）。
    pub fn with_defaults() -> Self {
        let mut r = Self::new();
        r.register_field(RangeValidator);
        r.register_field(NonNegativeValidator);
        r.register_field(SizeValidator);
        r.register_table(UniqueKeyValidator);
        r.register_table(SingleRecordValidator);
        r
    }

    pub fn register_field<V: IDataValidator + 'static>(&mut self, v: V) {
        self.field_validators.push(Box::new(v));
    }

    pub fn register_table<V: ITableValidator + 'static>(&mut self, v: V) {
        self.table_validators.push(Box::new(v));
    }
}

// ============================================================================
// 内置校验器
// ============================================================================

/// 整数/浮点范围校验（`range=[min,max]` 标签）。
#[derive(Debug, Default)]
pub struct RangeValidator;

impl IDataValidator for RangeValidator {
    fn name(&self) -> &str {
        "range"
    }

    fn validate(&self, value: &DType, type_info: &TypeInfo) -> Result<(), String> {
        let Some(range) = type_info.tags.get("range") else {
            return Ok(());
        };
        let (min, max) = parse_range(range)?;
        let v = match value {
            DType::Int(i) => *i as f64,
            DType::UInt(u) => *u as f64,
            DType::Float(f) => *f,
            DType::Null => return Ok(()), // 空值不校验范围（可空场景）
            other => {
                return Err(format!("range 校验不适用于类型 {}", other.type_name()));
            }
        };
        if v < min || v > max {
            return Err(format!(
                "值 {} 超出范围 [{}, {}]",
                v, min as i64, max as i64
            ));
        }
        Ok(())
    }
}

/// 非负校验（`nonneg` 标签）。
#[derive(Debug, Default)]
pub struct NonNegativeValidator;

impl IDataValidator for NonNegativeValidator {
    fn name(&self) -> &str {
        "nonneg"
    }

    fn validate(&self, value: &DType, type_info: &TypeInfo) -> Result<(), String> {
        let Some(tag) = type_info.tags.get("nonneg") else {
            return Ok(());
        };
        if tag.is_empty() {
            return Ok(());
        }
        match value {
            DType::Int(v) => {
                if *v < 0 {
                    Err(format!("值 {} 为负，不满足非负约束", v))
                } else {
                    Ok(())
                }
            }
            DType::UInt(_) => Ok(()),
            DType::Float(v) => {
                if *v < 0.0 {
                    Err(format!("值 {} 为负，不满足非负约束", v))
                } else {
                    Ok(())
                }
            }
            DType::Null => Ok(()),
            other => Err(format!("nonneg 校验不适用于类型 {}", other.type_name())),
        }
    }
}

/// 容器大小校验（`size=[min,max]` 标签）。
#[derive(Debug, Default)]
pub struct SizeValidator;

impl IDataValidator for SizeValidator {
    fn name(&self) -> &str {
        "size"
    }

    fn validate(&self, value: &DType, type_info: &TypeInfo) -> Result<(), String> {
        let Some(size) = type_info.tags.get("size") else {
            return Ok(());
        };
        let (min, max) = parse_int_range(size)?;
        let len = match value {
            DType::List(v) | DType::Set(v) | DType::Array(v) => v.len(),
            DType::Map(entries) => entries.len(),
            DType::Null => return Ok(()),
            other => {
                return Err(format!("size 校验不适用于类型 {}", other.type_name()));
            }
        };
        if (len as i64) < min || (len as i64) > max {
            return Err(format!("容器大小 {} 超出范围 [{}, {}]", len, min, max));
        }
        Ok(())
    }
}

/// 索引唯一校验（map / list 模式的索引字段值必须唯一）。
#[derive(Debug, Default)]
pub struct UniqueKeyValidator;

impl ITableValidator for UniqueKeyValidator {
    fn name(&self) -> &str {
        "unique-key"
    }

    fn validate(
        &self,
        table: &DefTable,
        data: &TableData,
        fields: &[(String, TypeInfo)],
    ) -> Vec<Diagnostic> {
        if table.mode == TableMode::One || table.index.is_empty() {
            return Vec::new();
        }
        let mut diags = Vec::new();
        // 每个索引列做唯一性检查
        for index in &table.index {
            let mut seen: HashSet<String> = HashSet::new();
            for (ri, record) in data.records.iter().enumerate() {
                // 找到索引列在 fields 中的位置（联合索引用多个字段拼接）
                let mut parts = Vec::new();
                for col in &index.columns {
                    let pos = fields.iter().position(|(name, _)| name == col);
                    let key = match pos.and_then(|p| record.data.get(p)) {
                        Some(v) => key_string(v),
                        None => format!("<missing {}>", col),
                    };
                    parts.push(key);
                }
                let composite = parts.join("|");
                if !seen.insert(composite.clone()) {
                    diags.push(Diagnostic::error(
                        format!("{}[行{}]", table.name, ri + 1),
                        format!("索引 {} 的值重复: {}", index.columns.join("+"), composite),
                    ));
                }
            }
        }
        diags
    }
}

/// one 模式恰好一条记录。
#[derive(Debug, Default)]
pub struct SingleRecordValidator;

impl ITableValidator for SingleRecordValidator {
    fn name(&self) -> &str {
        "single-record"
    }

    fn validate(
        &self,
        table: &DefTable,
        data: &TableData,
        _fields: &[(String, TypeInfo)],
    ) -> Vec<Diagnostic> {
        if table.mode == TableMode::One && data.len() != 1 {
            return vec![Diagnostic::error(
                &table.name,
                format!("one 模式表应有且仅有 1 条记录，实际 {} 条", data.len()),
            )];
        }
        Vec::new()
    }
}

// ============================================================================
// 校验入口
// ============================================================================

/// 校验一张表：返回所有诊断（字段级 + 表级）。
pub fn validate_table(
    table: &DefTable,
    data: &TableData,
    fields: &[(String, TypeInfo)],
    registry: &ValidatorRegistry,
) -> Vec<Diagnostic> {
    let mut diags = Vec::new();

    // 字段级
    for (ri, record) in data.records.iter().enumerate() {
        for (fi, (name, ti)) in fields.iter().enumerate() {
            let value = record.data.get(fi).unwrap_or(&DType::Null);
            for fv in &registry.field_validators {
                if let Err(e) = fv.validate(value, ti) {
                    diags.push(Diagnostic::error(
                        format!("{}[行{}].{}", table.name, ri + 1, name),
                        e,
                    ));
                }
            }
        }
    }

    // 表级
    for tv in &registry.table_validators {
        diags.extend(tv.validate(table, data, fields));
    }

    diags
}

// ============================================================================
// 辅助
// ============================================================================

/// 跨表外键校验：检查 ref 字段值存在于目标表的主键集合。
pub fn validate_foreign_keys(
    table_name: &str,
    _table: &DefTable,
    data: &TableData,
    fields: &[(String, TypeInfo)],
    key_sets: &std::collections::HashMap<String, HashSet<String>>,
) -> Vec<Diagnostic> {
    let mut diags = Vec::new();
    for (fi, (fname, fti)) in fields.iter().enumerate() {
        if let TypeKind::Ref(target) = &fti.kind {
            let Some(keys) = key_sets.get(target) else {
                continue;
            };
            for (ri, record) in data.records.iter().enumerate() {
                let val = record.data.get(fi).unwrap_or(&DType::Null);
                if val.is_null() {
                    continue;
                }
                let key = key_string(val);
                if !keys.contains(&key) {
                    diags.push(Diagnostic::error(
                        format!("{}[行{}].{}", table_name, ri + 1, fname),
                        format!("外键值 {} 不存在于表 '{}'", key, target),
                    ));
                }
            }
        }
    }
    diags
}

/// 解析 `[min,max]` 闭区间。
fn parse_range(s: &str) -> Result<(f64, f64), String> {
    let s = s.trim();
    let inner = s
        .strip_prefix('[')
        .and_then(|x| x.strip_suffix(']'))
        .ok_or_else(|| format!("range 格式应为 [min,max]，实际 '{}'", s))?;
    let parts: Vec<&str> = inner.split(',').collect();
    if parts.len() != 2 {
        return Err(format!("range 应含一个逗号: '{}'", s));
    }
    let min = parts[0]
        .trim()
        .parse::<f64>()
        .map_err(|_| format!("非法下界 '{}'", parts[0]))?;
    let max = parts[1]
        .trim()
        .parse::<f64>()
        .map_err(|_| format!("非法上界 '{}'", parts[1]))?;
    if min > max {
        return Err(format!("下界 {} 大于上界 {}", min, max));
    }
    Ok((min, max))
}

/// 解析 `[min,max]` 整数闭区间，用于容器大小校验。
fn parse_int_range(s: &str) -> Result<(i64, i64), String> {
    let s = s.trim();
    let inner = s
        .strip_prefix('[')
        .and_then(|x| x.strip_suffix(']'))
        .ok_or_else(|| format!("size 格式应为 [min,max]，实际 '{}'", s))?;
    let parts: Vec<&str> = inner.split(',').collect();
    if parts.len() != 2 {
        return Err(format!("size 应含一个逗号: '{}'", s));
    }
    let min = parts[0]
        .trim()
        .parse::<i64>()
        .map_err(|_| format!("非法下界 '{}'，应为非负整数", parts[0]))?;
    let max = parts[1]
        .trim()
        .parse::<i64>()
        .map_err(|_| format!("非法上界 '{}'，应为非负整数", parts[1]))?;
    if min < 0 || max < 0 {
        return Err("size 上下界应为非负整数".to_string());
    }
    if min > max {
        return Err(format!("下界 {} 大于上界 {}", min, max));
    }
    Ok((min, max))
}

/// 值 → 可比较的字符串 key（用于唯一性判断 / 外键索引）。
pub fn key_string(v: &DType) -> String {
    match v {
        DType::Int(i) => format!("i{}", i),
        DType::UInt(u) => format!("u{}", u),
        DType::Str(s) => format!("s{}", s),
        DType::Bool(b) => format!("b{}", b),
        DType::Enum(_, val) => format!("e{}", val),
        DType::Null => "null".to_string(),
        other => other.type_name().to_string(),
    }
}

// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::TypeKind;

    #[test]
    fn range_validator() {
        let v = RangeValidator;
        let ti = TypeInfo {
            kind: TypeKind::I32,
            nullable: false,
            tags: [("range".to_string(), "[0,100]".to_string())]
                .into_iter()
                .collect(),
        };
        assert!(v.validate(&DType::Int(50), &ti).is_ok());
        assert!(v.validate(&DType::Int(101), &ti).is_err());
        // 无 range 标签则跳过
        let ti2 = TypeInfo::new(TypeKind::I32);
        assert!(v.validate(&DType::Int(9999), &ti2).is_ok());
    }

    #[test]
    fn parse_range_basic() {
        assert_eq!(parse_range("[1,100]").unwrap(), (1.0, 100.0));
        assert!(parse_range("[100,1]").is_err());
        assert!(parse_range("abc").is_err());
    }

    #[test]
    fn non_negative_validator() {
        let v = NonNegativeValidator;
        let ti = TypeInfo {
            kind: TypeKind::I32,
            nullable: false,
            tags: [("nonneg".to_string(), "true".to_string())]
                .into_iter()
                .collect(),
        };

        assert_eq!(
            v.validate(&DType::Int(-1), &ti).unwrap_err(),
            "值 -1 为负，不满足非负约束"
        );
        assert!(v.validate(&DType::Int(0), &ti).is_ok());
        assert!(v.validate(&DType::Int(5), &ti).is_ok());
        assert_eq!(
            v.validate(&DType::Float(-0.5), &ti).unwrap_err(),
            "值 -0.5 为负，不满足非负约束"
        );
        assert!(v.validate(&DType::UInt(1), &ti).is_ok());
        assert!(v.validate(&DType::Null, &ti).is_ok());

        let err = v.validate(&DType::Str("x".to_string()), &ti).unwrap_err();
        assert!(err.contains("nonneg 校验不适用于类型"));

        let ti_without_tag = TypeInfo::new(TypeKind::I32);
        assert!(v.validate(&DType::Int(-1), &ti_without_tag).is_ok());
        assert!(v
            .validate(&DType::Str("x".to_string()), &ti_without_tag)
            .is_ok());
    }

    #[test]
    fn defaults_include_non_negative_validator() {
        let registry = ValidatorRegistry::with_defaults();
        assert!(registry
            .field_validators
            .iter()
            .any(|v| v.name() == "nonneg"));
    }

    #[test]
    fn size_validator() {
        let v = SizeValidator;
        let mut ti = TypeInfo {
            kind: TypeKind::List(Box::new(TypeInfo::new(TypeKind::I32))),
            nullable: false,
            tags: [("size".to_string(), "[1,3]".to_string())]
                .into_iter()
                .collect(),
        };
        let list = DType::List(vec![
            DType::Int(1),
            DType::Int(2),
            DType::Int(3),
        ]);
        assert!(v.validate(&list, &ti).is_ok());

        ti.tags.insert("size".to_string(), "[4,5]".to_string());
        assert_eq!(
            v.validate(&list, &ti).unwrap_err(),
            "容器大小 3 超出范围 [4, 5]"
        );

        ti.kind = TypeKind::Map(
            Box::new(TypeInfo::new(TypeKind::Str)),
            Box::new(TypeInfo::new(TypeKind::I32)),
        );
        ti.tags.insert("size".to_string(), "[2,2]".to_string());
        let map = DType::Map(vec![
            (DType::Str("a".to_string()), DType::Int(1)),
            (DType::Str("b".to_string()), DType::Int(2)),
        ]);
        assert!(v.validate(&map, &ti).is_ok());

        assert!(v.validate(&DType::Null, &ti).is_ok());

        ti.tags.remove("size");
        assert!(v.validate(&DType::Str("x".to_string()), &ti).is_ok());
        ti.tags.insert("size".to_string(), "[0,1]".to_string());
        assert_eq!(
            v.validate(&DType::Str("x".to_string()), &ti).unwrap_err(),
            "size 校验不适用于类型 string"
        );

        ti.tags.insert("size".to_string(), "[3,1]".to_string());
        assert!(v.validate(&list, &ti).is_err());
    }
}
