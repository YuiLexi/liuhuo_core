//! 导出：`IExporter` trait + 内置 `JsonDataExporter` / `JsonSchemaExporter`。
//!
//! trait + 注册表扩展点。数据导出把 `TableData` 物化为目标格式。

use crate::defs::{DefKind, DefValue, TableMode};
use crate::value::{DType, DataContext, Record, TableData};
use serde_json::{Map, Value};

/// 导出器接口。
pub trait IExporter: std::fmt::Debug + Send + Sync {
    fn name(&self) -> &str;
}

// ============================================================================
// 数据导出
// ============================================================================

/// JSON 数据导出器：map → 对象、list → 数组、one → 单对象。
#[derive(Debug, Default)]
pub struct JsonDataExporter;

impl IExporter for JsonDataExporter {
    fn name(&self) -> &str {
        "json-data"
    }
}

impl JsonDataExporter {
    /// 导出单张表。
    pub fn export_table(
        &self,
        table: &crate::defs::DefTable,
        data: &TableData,
        ctx: &dyn DataContext,
    ) -> Value {
        match table.mode {
            TableMode::One => {
                // 单对象
                data.records
                    .first()
                    .map(|r| record_to_json(r, ctx))
                    .unwrap_or(Value::Null)
            }
            TableMode::Map => {
                // 对象：key（索引字段值）→ 记录
                let mut map = Map::new();
                for record in &data.records {
                    let key = index_key_string(table, record);
                    map.insert(key, record_to_json(record, ctx));
                }
                Value::Object(map)
            }
            TableMode::List => {
                // 数组
                Value::Array(
                    data.records
                        .iter()
                        .map(|r| record_to_json(r, ctx))
                        .collect(),
                )
            }
        }
    }
}

/// 单条记录 → JSON 对象（字段名通过 DataContext 查询）。
pub fn record_to_json(record: &Record, ctx: &dyn DataContext) -> Value {
    let bean = record.bean.as_deref().unwrap_or("");
    let fields = ctx.bean_fields(bean).unwrap_or_default();
    let mut map = Map::new();
    for (i, v) in record.data.iter().enumerate() {
        let key = fields.get(i).cloned().unwrap_or_else(|| format!("f{}", i));
        map.insert(key, v.to_json(ctx));
    }
    Value::Object(map)
}

/// 取 map 模式记录的主键字符串（第一个索引字段的值）。
fn index_key_string(table: &crate::defs::DefTable, record: &Record) -> String {
    if let Some(first_index) = table.index.first()
        && let Some(col) = first_index.columns.first()
    {
        // 找字段位置（需要字段名，这里用记录内按顺序的近似：无法直接定位，退回行号）
        let _ = col;
    }
    // 无索引信息时，用第一条标量值作为 key；否则用行序号兜底
    record
        .data
        .first()
        .map(value_key)
        .unwrap_or_else(|| "?".to_string())
}

fn value_key(v: &DType) -> String {
    match v {
        DType::Int(i) => i.to_string(),
        DType::UInt(u) => u.to_string(),
        DType::Str(s) => s.clone(),
        DType::Enum(_, val) => val.to_string(),
        DType::Bool(b) => b.to_string(),
        other => format!("{:?}", other),
    }
}

// ============================================================================
// Schema 导出
// ============================================================================

/// JSON Schema 导出器：把编译后的定义集导出为 schema.json。
#[derive(Debug, Default)]
pub struct JsonSchemaExporter;

impl IExporter for JsonSchemaExporter {
    fn name(&self) -> &str {
        "json-schema"
    }
}

impl JsonSchemaExporter {
    /// 导出定义集（枚举 / Bean / 表 / 记录）为 JSON。
    pub fn export(&self, defs: &[&DefValue]) -> Value {
        let mut enums = Vec::new();
        let mut beans = Vec::new();
        let mut tables = Vec::new();
        let mut records = Vec::new();

        for def in defs {
            match def {
                DefValue::Enum(e) => enums.push(serde_json::json!({
                    "name": e.name,
                    "items": e.items.iter().map(|i| serde_json::json!({
                        "name": i.name, "value": i.value, "alias": i.alias,
                    })).collect::<Vec<_>>(),
                })),
                DefValue::Bean(b) => beans.push(serde_json::json!({
                    "name": b.name,
                    "parent": b.parent,
                    "fields": b.fields.iter().map(|f| serde_json::json!({
                        "name": f.name, "type": f.type_str, "comment": f.comment,
                    })).collect::<Vec<_>>(),
                })),
                DefValue::Table(t) => tables.push(serde_json::json!({
                    "name": t.name,
                    "mode": match t.mode { TableMode::One => "one", TableMode::Map => "map", TableMode::List => "list" },
                    "value_type": t.value_type,
                    "index": t.index.iter().map(|i| i.columns.join("+")).collect::<Vec<_>>().join(","),
                })),
                DefValue::Record(r) => records.push(serde_json::json!({
                    "name": r.name,
                    "fields": r.fields.iter().map(|f| serde_json::json!({
                        "name": f.name, "type": f.type_str,
                    })).collect::<Vec<_>>(),
                })),
            }
        }

        serde_json::json!({
            "enums": enums,
            "beans": beans,
            "tables": tables,
            "records": records,
        })
    }

    /// 从符号表导出（按 kind 过滤）。
    pub fn export_kind(&self, defs: &[&DefValue], kind: DefKind) -> Value {
        let filtered: Vec<&DefValue> = defs.iter().copied().filter(|d| d.kind() == kind).collect();
        self.export(&filtered)
    }
}
