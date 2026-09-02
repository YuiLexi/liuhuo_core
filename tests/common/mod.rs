//! 集成测试共享辅助：定位 fixtures、从 JSON 文件加载定义。
//!
//! 该模块被多个测试 crate 各自编译，某些 crate 不会用到全部函数，故放宽 dead_code。

#![allow(dead_code)]

use liuhuo_core::RawDef;
use std::fs;
use std::path::PathBuf;

/// fixtures 根目录。
pub fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
}

/// 从 fixtures 下的相对路径（如 "beans/ItemCfg.json"）加载定义，
/// 按第一段目录名（enums/beans/tables/records）决定反序列化类型。
pub fn load_raw(rel: &str) -> RawDef {
    let path = fixtures_dir().join(rel);
    let s =
        fs::read_to_string(&path).unwrap_or_else(|e| panic!("读取 {} 失败: {}", path.display(), e));
    match rel.split('/').next().unwrap_or("") {
        "enums" => RawDef::Enum(
            serde_json::from_str(&s).unwrap_or_else(|e| panic!("解析 {} 失败: {}", rel, e)),
        ),
        "beans" => RawDef::Bean(
            serde_json::from_str(&s).unwrap_or_else(|e| panic!("解析 {} 失败: {}", rel, e)),
        ),
        "tables" => RawDef::Table(
            serde_json::from_str(&s).unwrap_or_else(|e| panic!("解析 {} 失败: {}", rel, e)),
        ),
        "records" => RawDef::Record(
            serde_json::from_str(&s).unwrap_or_else(|e| panic!("解析 {} 失败: {}", rel, e)),
        ),
        _ => panic!("未知定义类型目录: {}", rel),
    }
}

/// 加载完整项目定义集（enums → beans → records → tables，保持确定性顺序）。
pub fn load_all_project_defs() -> Vec<RawDef> {
    let mut defs = Vec::new();
    for kind in ["enums", "beans", "records", "tables"] {
        let dir = fixtures_dir().join(kind);
        let mut entries: Vec<_> = fs::read_dir(&dir)
            .unwrap_or_else(|e| panic!("读取目录 {} 失败: {}", dir.display(), e))
            .filter_map(|e| e.ok())
            .collect();
        entries.sort_by_key(|e| e.file_name());
        for entry in entries {
            let fname = entry.file_name().to_string_lossy().to_string();
            if fname.ends_with(".json") {
                defs.push(load_raw(&format!("{}/{}", kind, fname)));
            }
        }
    }
    defs
}
