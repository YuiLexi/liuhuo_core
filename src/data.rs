//! 数据加载：`IDataLoader` trait + `DataLoaderRegistry` + `JsonDataLoader`。
//!
//! 设计：trait + 注册表（按扩展名路由），核心管线零硬编码。
//! 加载返回 `Result<TableData, String>`；解码即校验（类型不匹配、非法枚举值都报错并带字段路径）。

use crate::defs::DefTable;
use crate::types::{TypeInfo, TypeKind};
use crate::value::{DType, DataContext, Record, TableData};
use serde_json::Value;
use std::path::Path;

/// 数据加载器接口。
pub trait IDataLoader: std::fmt::Debug + Send + Sync {
    /// 加载器名称（如 "json"）。
    fn name(&self) -> &str;
    /// 支持的扩展名（不含点）。
    fn extensions(&self) -> &[&str];
    /// 加载一张表的数据文件。
    fn load_table(
        &self,
        path: &Path,
        table: &DefTable,
        ctx: &dyn DataContext,
    ) -> Result<TableData, String>;
}

/// 数据加载器注册表（按扩展名路由）。
#[derive(Debug, Default)]
pub struct DataLoaderRegistry {
    loaders: Vec<Box<dyn IDataLoader>>,
}

impl DataLoaderRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register<L: IDataLoader + 'static>(&mut self, loader: L) {
        self.loaders.push(Box::new(loader));
    }

    pub fn find(&self, ext: &str) -> Option<&dyn IDataLoader> {
        self.loaders
            .iter()
            .find(|l| l.extensions().contains(&ext))
            .map(|v| &**v)
    }
}

/// 按文件扩展名加载一张表。
pub fn load_table_from_path(
    path: &Path,
    table: &DefTable,
    ctx: &dyn DataContext,
    registry: &DataLoaderRegistry,
) -> Result<TableData, String> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("json");
    let loader = registry
        .find(ext)
        .ok_or_else(|| format!("无支持 '.{}' 的数据加载器", ext))?;
    loader.load_table(path, table, ctx)
}

// ============================================================================
// JsonDataLoader
// ============================================================================

/// JSON 数据加载器。
#[derive(Debug, Default)]
pub struct JsonDataLoader;

impl IDataLoader for JsonDataLoader {
    fn name(&self) -> &str {
        "json"
    }

    fn extensions(&self) -> &[&str] {
        &["json"]
    }

    fn load_table(
        &self,
        path: &Path,
        table: &DefTable,
        ctx: &dyn DataContext,
    ) -> Result<TableData, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("无法读取数据文件 '{}': {}", path.display(), e))?;
        let value: Value = serde_json::from_str(&text)
            .map_err(|e| format!("JSON 解析失败 '{}': {}", path.display(), e))?;

        // 支持 [ {..}, .. ] / { "rows": [..] } / 单对象（one 模式）
        let rows: Vec<Value> = match value {
            Value::Array(arr) => arr,
            Value::Object(ref obj) if obj.contains_key("rows") => match &obj["rows"] {
                Value::Array(arr) => arr.clone(),
                other => return Err(format!("rows 必须是数组，实际为 {}", other)),
            },
            other => vec![other],
        };

        let mut data = TableData::with_capacity(rows.len());
        for (i, row) in rows.iter().enumerate() {
            let record = decode_record(row, &table.value_type, ctx)
                .map_err(|e| format!("表 '{}' 第 {} 条: {}", table.name, i + 1, e))?;
            data.push(record);
        }
        Ok(data)
    }
}

// ============================================================================
// 解码
// ============================================================================

/// 解码一条记录（Bean 对象）。
fn decode_record(value: &Value, bean_name: &str, ctx: &dyn DataContext) -> Result<Record, String> {
    let obj = value
        .as_object()
        .ok_or_else(|| "记录必须是 JSON 对象".to_string())?;

    // 多态：$type 指定实际 Bean 类型
    let actual_bean = obj
        .get("$type")
        .and_then(Value::as_str)
        .unwrap_or(bean_name)
        .to_string();

    let fields = ctx
        .bean_hierarchy_fields(&actual_bean)
        .ok_or_else(|| format!("未知 Bean 类型 '{}'", actual_bean))?;

    let mut record = Record::with_capacity(fields.len());
    record.bean = Some(actual_bean.clone());
    for (name, ti) in &fields {
        let v = obj.get(name).unwrap_or(&Value::Null);
        record.push(decode_value(v, ti, ctx).map_err(|e| format!("字段 '{}': {}", name, e))?);
    }
    Ok(record)
}

/// 解码单个值（递归，含校验）。
fn decode_value(value: &Value, ti: &TypeInfo, ctx: &dyn DataContext) -> Result<DType, String> {
    if value.is_null() {
        return Ok(DType::Null);
    }

    match &ti.kind {
        TypeKind::Bool => value
            .as_bool()
            .map(DType::Bool)
            .ok_or_else(|| expected("bool", value)),
        TypeKind::I8 | TypeKind::I16 | TypeKind::I32 | TypeKind::I64 => value
            .as_i64()
            .map(DType::Int)
            .ok_or_else(|| expected("有符号整数", value)),
        TypeKind::U8 | TypeKind::U16 | TypeKind::U32 | TypeKind::U64 => value
            .as_u64()
            .map(DType::UInt)
            .ok_or_else(|| expected("无符号整数", value)),
        TypeKind::F32 | TypeKind::F64 => value
            .as_f64()
            .map(DType::Float)
            .ok_or_else(|| expected("浮点数", value)),
        TypeKind::Str => value
            .as_str()
            .map(|s| DType::Str(s.to_string()))
            .ok_or_else(|| expected("字符串", value)),
        TypeKind::Text => value
            .as_str()
            .map(|s| DType::Text(s.to_string()))
            .ok_or_else(|| expected("字符串", value)),
        TypeKind::DateTime => value
            .as_i64()
            .map(DType::DateTime)
            .ok_or_else(|| expected("Unix 时间戳", value)),
        TypeKind::Enum(name) => {
            let v = match value {
                Value::Number(n) => n.as_i64().ok_or_else(|| "枚举值必须是整数".to_string())?,
                Value::String(s) => ctx
                    .enum_value(name, s)
                    .ok_or_else(|| format!("枚举 '{}' 不包含 '{}'", name, s))?,
                other => return Err(expected("枚举名或整数", other)),
            };
            Ok(DType::Enum(name.clone(), v))
        }
        TypeKind::Bean(name) => {
            let rec = decode_record(value, name, ctx)?;
            Ok(DType::Bean(name.clone(), rec.data))
        }
        TypeKind::Ref(_) => value
            .as_i64()
            .map(DType::Int)
            .ok_or_else(|| expected("整数（外键）", value)),
        TypeKind::Unresolved(name) => Err(format!("类型 '{}' 未解析，无法解码", name)),
        TypeKind::Array(elem) | TypeKind::List(elem) | TypeKind::Set(elem) => {
            let arr = value.as_array().ok_or_else(|| expected("数组", value))?;
            let vals = arr
                .iter()
                .map(|x| decode_value(x, elem, ctx))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(match &ti.kind {
                TypeKind::Array(_) => DType::Array(vals),
                TypeKind::Set(_) => DType::Set(vals),
                _ => DType::List(vals),
            })
        }
        TypeKind::Map(key_ti, val_ti) => {
            let entries = match value {
                Value::Object(obj) => obj
                    .iter()
                    .map(|(k, v)| {
                        let key = decode_map_key(k, key_ti, ctx)?;
                        let val = decode_value(v, val_ti, ctx)?;
                        Ok((key, val))
                    })
                    .collect::<Result<Vec<_>, String>>()?,
                Value::Array(arr) => arr
                    .iter()
                    .map(|e| {
                        let pair = e
                            .as_array()
                            .filter(|p| p.len() == 2)
                            .ok_or_else(|| "Map 条目必须是 [key, value]".to_string())?;
                        Ok((
                            decode_value(&pair[0], key_ti, ctx)?,
                            decode_value(&pair[1], val_ti, ctx)?,
                        ))
                    })
                    .collect::<Result<Vec<_>, String>>()?,
                other => return Err(expected("对象或 [key, value] 数组", other)),
            };
            Ok(DType::Map(entries))
        }
    }
}

/// 解码 Map 的 key（从 JSON 对象的字符串键）。
fn decode_map_key(
    key_str: &str,
    key_ti: &TypeInfo,
    ctx: &dyn DataContext,
) -> Result<DType, String> {
    match &key_ti.kind {
        TypeKind::Bool => key_str
            .parse::<bool>()
            .map(DType::Bool)
            .map_err(|_| format!("非法 bool key '{}'", key_str)),
        TypeKind::I8 | TypeKind::I16 | TypeKind::I32 | TypeKind::I64 => key_str
            .parse::<i64>()
            .map(DType::Int)
            .map_err(|_| format!("非法整数 key '{}'", key_str)),
        TypeKind::U8 | TypeKind::U16 | TypeKind::U32 | TypeKind::U64 => key_str
            .parse::<u64>()
            .map(DType::UInt)
            .map_err(|_| format!("非法无符号整数 key '{}'", key_str)),
        TypeKind::Str => Ok(DType::Str(key_str.to_string())),
        TypeKind::Enum(name) => ctx
            .enum_value(name, key_str)
            .map(|v| DType::Enum(name.clone(), v))
            .ok_or_else(|| format!("枚举 '{}' 不包含 '{}'", name, key_str)),
        _ => Err(format!("类型 '{}' 不能作为 map key", key_ti.type_name())),
    }
}

fn expected(exp: &str, actual: &Value) -> String {
    format!("期望 JSON {}，实际为 {}", exp, actual)
}

// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SymbolTable;
    use crate::defs::RawBean;
    use crate::defs::RawDef;
    use crate::defs::RawField;

    fn build_symbol_table() -> SymbolTable {
        // 复用 fixtures 的 ItemCfg（引用 Quality 枚举）
        let mut s = SymbolTable::new();
        let quality = serde_json::from_str::<crate::RawEnum>(
            r#"{"name":"Quality","items":[{"name":"White","value":"0"},{"name":"Green","value":"1"}]}"#,
        )
        .unwrap();
        s.register(&RawDef::Enum(quality));
        let item = serde_json::from_str::<crate::RawBean>(
            r#"{"name":"ItemCfg","module":"game","fields":[{"name":"id","type":"int"},{"name":"name","type":"string"},{"name":"quality","type":"Quality"},{"name":"attrs","type":"map<string,int>"}]}"#,
        )
        .unwrap();
        s.register(&RawDef::Bean(item));
        s
    }

    #[test]
    fn decode_json_rows() {
        let s = build_symbol_table();
        let table = crate::DefTable {
            name: "TbItem".into(),
            module: "game".into(),
            comment: None,
            mode: crate::TableMode::Map,
            index: vec![],
            value_type: "game.ItemCfg".into(),
            input: vec![],
            groups: vec![],
        };
        let json = serde_json::json!([
            {"id": 1, "name": "药水", "quality": "Green", "attrs": {"hp": 10}},
            {"id": 2, "name": "铁剑", "quality": 0, "attrs": {"atk": 5}}
        ]);
        let path = std::env::temp_dir().join("liuhuo_test_tbitem.json");
        std::fs::write(&path, json.to_string()).unwrap();
        let loader = JsonDataLoader;
        let data = loader.load_table(&path, &table, &s).unwrap();
        assert_eq!(data.len(), 2);
        // 第一条记录：id=1, name="药水", quality=Green(1), attrs=map
        assert_eq!(data.records[0].data[0], DType::Int(1));
        assert_eq!(data.records[0].data[1], DType::Str("药水".into()));
        assert_eq!(data.records[0].data[2], DType::Enum("Quality".into(), 1));
        assert!(matches!(data.records[0].data[3], DType::Map(_)));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn decode_invalid_enum_errs() {
        let s = build_symbol_table();
        let raw = RawBean {
            name: "Item".into(),
            fields: vec![RawField {
                name: "q".into(),
                r#type: "Quality".into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        // 用 decode_value 直接测（通过加载）
        let _ = raw;
        // 非法枚举值
        let ti = crate::parse_type("Quality", &s).unwrap();
        let bad = serde_json::json!("NotARealValue");
        assert!(decode_value(&bad, &ti, &s).is_err());
    }
}
