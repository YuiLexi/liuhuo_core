//! 运行时数据值系统：`DType`（数据值）/ `Record`（行）/ `TableData`（表）。
//!
//! 与 [`crate::types::TypeInfo`] 形成"类型描述 vs 类型值"的对称关系：
//! `TypeInfo` 描述数据长什么样，`DType` 承载实际数据值。
//!
//! # 统一数值
//!
//! `DType` 用统一的 `Int(i64)` / `UInt(u64)` / `Float(f64)` 表示所有数值，
//! 位宽约束由 `TypeInfo` 在校验阶段检查，导出时由目标格式负责截断。

use serde_json::{Map, Value};
use std::collections::HashMap;

// ============================================================================
// DType —— 统一数据值枚举
// ============================================================================

/// 运行时数据值。
#[derive(Debug, Clone, PartialEq)]
pub enum DType {
    Null,
    Bool(bool),
    /// 有符号整数（位宽由 TypeInfo 约束）
    Int(i64),
    /// 无符号整数（位宽由 TypeInfo 约束）
    UInt(u64),
    /// 浮点数（精度由 TypeInfo 约束）
    Float(f64),
    /// 日期时间（Unix 秒）
    DateTime(i64),
    Str(String),
    Text(String),
    /// 枚举（full_name, 数值）
    Enum(String, i64),
    /// Bean（full_name, 按层级字段顺序的值）
    Bean(String, Vec<DType>),
    Array(Vec<DType>),
    List(Vec<DType>),
    Set(Vec<DType>),
    Map(Vec<(DType, DType)>),
}

impl DType {
    pub fn is_null(&self) -> bool {
        matches!(self, DType::Null)
    }

    pub fn type_name(&self) -> &'static str {
        match self {
            DType::Null => "null",
            DType::Bool(_) => "bool",
            DType::Int(_) => "int",
            DType::UInt(_) => "uint",
            DType::Float(_) => "float",
            DType::DateTime(_) => "datetime",
            DType::Str(_) => "string",
            DType::Text(_) => "text",
            DType::Enum(_, _) => "enum",
            DType::Bean(_, _) => "bean",
            DType::Array(_) => "array",
            DType::List(_) => "list",
            DType::Set(_) => "set",
            DType::Map(_) => "map",
        }
    }

    /// 转为 JSON（导出用）。Bean 字段名通过 `DataContext` 查询。
    pub fn to_json(&self, ctx: &dyn DataContext) -> Value {
        match self {
            DType::Null => Value::Null,
            DType::Bool(b) => Value::Bool(*b),
            DType::Int(i) => Value::Number((*i).into()),
            DType::UInt(u) => Value::Number((*u).into()),
            DType::Float(f) => serde_json::Number::from_f64(*f)
                .map(Value::Number)
                .unwrap_or(Value::Null),
            DType::DateTime(d) => Value::Number((*d).into()),
            DType::Str(s) | DType::Text(s) => Value::String(s.clone()),
            DType::Enum(_, v) => Value::Number((*v).into()),
            DType::Bean(name, fields) => {
                let names = ctx.bean_fields(name).unwrap_or_default();
                let mut map = Map::new();
                for (i, f) in fields.iter().enumerate() {
                    let key = names.get(i).cloned().unwrap_or_else(|| format!("f{}", i));
                    map.insert(key, f.to_json(ctx));
                }
                Value::Object(map)
            }
            DType::Array(v) | DType::List(v) | DType::Set(v) => {
                Value::Array(v.iter().map(|x| x.to_json(ctx)).collect())
            }
            DType::Map(entries) => {
                let mut map = Map::new();
                for (k, v) in entries {
                    let key = match k {
                        DType::Str(s) => s.clone(),
                        other => format!("{}_{}", other.type_name(), other.to_json(ctx)),
                    };
                    map.insert(key, v.to_json(ctx));
                }
                Value::Object(map)
            }
        }
    }
}

// ============================================================================
// Record / TableData
// ============================================================================

/// 单行数据记录。
#[derive(Debug, Clone, Default)]
pub struct Record {
    /// 实际 Bean 类型（多态时非空）。
    pub bean: Option<String>,
    /// 行数据值，按层级字段顺序存储。
    pub data: Vec<DType>,
    /// 行级标签。
    pub tags: HashMap<String, String>,
}

impl Record {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_capacity(cap: usize) -> Self {
        Self {
            bean: None,
            data: Vec::with_capacity(cap),
            tags: HashMap::new(),
        }
    }

    pub fn push(&mut self, value: DType) {
        self.data.push(value);
    }

    pub fn len(&self) -> usize {
        self.data.len()
    }

    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }
}

/// 表数据：一个表的所有行。
#[derive(Debug, Clone, Default)]
pub struct TableData {
    pub records: Vec<Record>,
}

impl TableData {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_capacity(cap: usize) -> Self {
        Self {
            records: Vec::with_capacity(cap),
        }
    }

    pub fn push(&mut self, record: Record) {
        self.records.push(record);
    }

    pub fn len(&self) -> usize {
        self.records.len()
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// 分页窗口：返回 (总行数, 窗口行)。用于前端虚拟滚动按需拉取。
    pub fn get_rows(&self, offset: usize, limit: usize) -> (usize, Vec<&Record>) {
        let total = self.records.len();
        let start = offset.min(total);
        let end = (offset + limit).min(total);
        (total, self.records[start..end].iter().collect())
    }
}

/// 更新单个单元格（单点更新，前端只重渲染该格）。
pub fn update_cell(
    data: &mut TableData,
    row: usize,
    field: usize,
    value: DType,
) -> Result<(), String> {
    let record = data
        .records
        .get_mut(row)
        .ok_or_else(|| format!("行号 {} 越界", row))?;
    if field >= record.data.len() {
        return Err(format!("字段号 {} 越界", field));
    }
    record.data[field] = value;
    Ok(())
}

// ============================================================================
// DataContext —— 数据加载/导出时查询定义
// ============================================================================

/// 数据上下文：加载/导出数据时查询符号表中的枚举与 Bean 字段。
pub trait DataContext {
    /// 解析枚举值：名称 / 别名 / 数值字符串 → i64。
    fn enum_value(&self, enum_name: &str, value: &str) -> Option<i64>;

    /// Bean 的层级字段名（含父类，从根到自身）。
    fn bean_fields(&self, bean_name: &str) -> Option<Vec<String>>;

    /// Bean 的层级字段（含父类），完整字段名 + 类型。用于解码 Bean 值。
    fn bean_hierarchy_fields(
        &self,
        bean_name: &str,
    ) -> Option<Vec<(String, crate::types::TypeInfo)>> {
        let _ = bean_name;
        None
    }
}

// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    struct Ctx;
    impl DataContext for Ctx {
        fn enum_value(&self, _: &str, _: &str) -> Option<i64> {
            None
        }
        fn bean_fields(&self, _: &str) -> Option<Vec<String>> {
            None
        }
    }

    #[test]
    fn to_json_scalars() {
        let ctx = Ctx;
        assert_eq!(DType::Int(42).to_json(&ctx), Value::from(42));
        assert_eq!(DType::Str("hi".into()).to_json(&ctx), Value::from("hi"));
        assert_eq!(
            DType::List(vec![DType::Int(1), DType::Int(2)]).to_json(&ctx),
            serde_json::json!([1, 2])
        );
        assert_eq!(DType::Null.to_json(&ctx), Value::Null);
    }

    #[test]
    fn pagination_and_cell_update() {
        let mut data = TableData::new();
        for i in 0..10 {
            let mut r = Record::new();
            r.data = vec![DType::Int(i), DType::Int(i * 2)];
            data.push(r);
        }
        let (total, rows) = data.get_rows(0, 3);
        assert_eq!(total, 10);
        assert_eq!(rows.len(), 3);
        let (_, rows2) = data.get_rows(8, 5);
        assert_eq!(rows2.len(), 2); // 只到末尾

        update_cell(&mut data, 0, 1, DType::Int(999)).unwrap();
        assert_eq!(data.records[0].data[1], DType::Int(999));
        assert!(update_cell(&mut data, 100, 0, DType::Int(0)).is_err());
    }
}
