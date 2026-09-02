//! 本地化（l10n 抽词）+ 路径校验。
//!
//! - `extract_localization`：从 text 类型字段抽取文本，生成 key → 文本（静态本地化抽词）。
//! - `validate_path`：路径字段校验（Godot/Unity 资源路径）。

use crate::types::{TypeInfo, TypeKind};
use crate::value::{DType, TableData};
use std::path::Path;

/// 从表数据抽取本地化文本。
///
/// 返回 `(key, 文本)` 列表。key 格式 `{表名}.{字段}.{行}`。
/// 只抽取 `text` 类型字段（本地化语义的文本）。
pub fn extract_localization(
    table_name: &str,
    data: &TableData,
    fields: &[(String, TypeInfo)],
) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for (fi, (fname, fti)) in fields.iter().enumerate() {
        if !matches!(fti.kind, TypeKind::Text) {
            continue;
        }
        for (ri, record) in data.records.iter().enumerate() {
            if let Some(DType::Text(t)) | Some(DType::Str(t)) = record.data.get(fi)
                && !t.trim().is_empty()
            {
                let key = format!("{}.{}.{}", table_name, fname, ri + 1);
                out.push((key, t.clone()));
            }
        }
    }
    out
}

/// 生成本地化 JSON（按语言分组）。
pub fn to_l10n_json(entries: &[(String, String)], lang: &str) -> serde_json::Value {
    let mut map = serde_json::Map::new();
    for (key, text) in entries {
        map.insert(key.clone(), serde_json::Value::String(text.clone()));
    }
    serde_json::json!({ "language": lang, "entries": map })
}

// ============================================================================
// 路径校验
// ============================================================================

/// 路径校验：检查字符串值指向的资源在 `root` 下存在。
///
/// `kind` 为资源类型标记（"godot" / "unity" / 通用），当前用于错误消息。
pub fn validate_path(value: &DType, kind: &str, root: &Path) -> Result<(), String> {
    let s = match value {
        DType::Str(s) | DType::Text(s) => s,
        DType::Null => return Ok(()),
        other => return Err(format!("路径字段期望字符串，实际 {}", other.type_name())),
    };
    let path = if s.starts_with("res://") {
        // Godot 资源路径
        root.join(s.trim_start_matches("res://"))
    } else {
        root.join(s)
    };
    if path.exists() {
        Ok(())
    } else {
        Err(format!(
            "{} 资源 '{}' 不存在（解析为 {}）",
            kind,
            s,
            path.display()
        ))
    }
}

/// 提取所有 `path(kind=...)` 字段并校验。
pub fn validate_paths(
    data: &TableData,
    fields: &[(String, TypeInfo)],
    root: &Path,
) -> Vec<(usize, String, String)> {
    let mut errors = Vec::new();
    for (fi, (fname, fti)) in fields.iter().enumerate() {
        let Some(kind) = fti.tags.get("path") else {
            continue;
        };
        for (ri, record) in data.records.iter().enumerate() {
            if let Some(v) = record.data.get(fi)
                && let Err(e) = validate_path(v, kind, root)
            {
                errors.push((ri + 1, fname.clone(), e));
            }
        }
    }
    errors
}

// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Record;

    #[test]
    fn extract_text_fields() {
        let fields = vec![
            ("id".to_string(), TypeInfo::new(TypeKind::I32)),
            ("name".to_string(), TypeInfo::new(TypeKind::Text)),
            ("desc".to_string(), TypeInfo::new(TypeKind::Str)),
        ];
        let mut data = TableData::new();
        let mut r = Record::new();
        r.data = vec![
            DType::Int(1),
            DType::Text("药水".into()),
            DType::Str("普通".into()),
        ];
        data.push(r);

        let entries = extract_localization("TbItem", &data, &fields);
        // 只有 Text 字段被抽取
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].0, "TbItem.name.1");
        assert_eq!(entries[0].1, "药水");
    }

    #[test]
    fn to_l10n_json_shape() {
        let entries = vec![("k".to_string(), "v".to_string())];
        let json = to_l10n_json(&entries, "zh-CN");
        assert_eq!(json["language"], "zh-CN");
        assert_eq!(json["entries"]["k"], "v");
    }

    #[test]
    fn path_validation() {
        let tmp = std::env::temp_dir();
        let exists = DType::Str(".".to_string());
        assert!(validate_path(&exists, "godot", &tmp).is_ok());
        let missing = DType::Str("__no_such_file__.png".to_string());
        assert!(validate_path(&missing, "godot", &tmp).is_err());
    }
}
