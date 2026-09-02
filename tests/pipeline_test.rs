//! P0 全量管线集成测试：创建项目 → 写 schema → 写数据 → 编译 → 校验 → 导出。
//!
//! 运行：`cargo test --test pipeline_test`

use liuhuo_core::{
    ProjectInfo, SymbolTable, compile_project, create_project, read_config, write_project_file,
};
use std::path::{Path, PathBuf};

/// 并行测试下保证每个测试独占一个临时根目录，避免互相 remove_dir_all 竞态。
static ROOT_COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// 临时根目录（create_project 的 parent）。
fn temp_root() -> PathBuf {
    let n = ROOT_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("liuhuo_pipeline_{}_{}", std::process::id(), n));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// 建一个完整小项目：Quality 枚举 + ItemCfg Bean + TbItem 表 + 数据文件。
/// 返回项目目录（root/name）。
fn build_project(root: &Path, name: &str) -> PathBuf {
    create_project(root, name).unwrap();
    let dir = root.join(name);

    write_project_file(
        &dir,
        "schemas/enums/Quality.json",
        r#"{"name":"Quality","items":[{"name":"White","value":"0"},{"name":"Green"},{"name":"Blue"}]}"#,
    )
    .unwrap();

    write_project_file(
        &dir,
        "schemas/beans/ItemCfg.json",
        r#"{"name":"ItemCfg","module":"game","fields":[{"name":"id","type":"int"},{"name":"name","type":"string"},{"name":"quality","type":"Quality"},{"name":"price","type":"int(range=[0,9999])"}]}"#,
    )
    .unwrap();

    write_project_file(
        &dir,
        "schemas/tables/TbItem.json",
        r#"{"name":"TbItem","module":"game","mode":"map","index":"id","value_type":"game.ItemCfg","input":["item.json"]}"#,
    )
    .unwrap();

    write_project_file(
        &dir,
        "datas/item.json",
        r#"[{"id":1,"name":"药水","quality":"Green","price":100},{"id":2,"name":"铁剑","quality":0,"price":500}]"#,
    )
    .unwrap();

    dir
}

#[test]
fn full_pipeline_compiles_clean() {
    let root = temp_root();
    let dir = build_project(&root, "clean");
    let config = read_config(&dir).unwrap();
    let outcome = compile_project(&dir, &config);
    assert!(outcome.is_ok(), "应无诊断: {:?}", outcome.diagnostics);
    assert_eq!(outcome.table_count, 1);
    assert_eq!(outcome.total_records, 2);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn data_validation_catches_range_and_unique() {
    let root = temp_root();
    let dir = build_project(&root, "bad");
    // 修改数据：price 超范围 + id 重复
    write_project_file(
        &dir,
        "datas/item.json",
        r#"[{"id":1,"name":"药水","quality":"Green","price":99999},{"id":1,"name":"重复id","quality":"Blue","price":10}]"#,
    )
    .unwrap();
    let config = read_config(&dir).unwrap();
    let outcome = compile_project(&dir, &config);
    assert!(!outcome.is_ok(), "应报错");
    assert!(
        outcome.data_error_count >= 2,
        "至少 range 越界 + id 重复: {:?}",
        outcome.diagnostics
    );
    assert!(
        outcome
            .diagnostics
            .iter()
            .any(|d| d.message.contains("超出范围"))
    );
    assert!(
        outcome
            .diagnostics
            .iter()
            .any(|d| d.message.contains("重复"))
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn project_create_then_open() {
    let root = temp_root();
    let dir = build_project(&root, "open");
    // 打开：读标识 + 配置 + 树
    let info = ProjectInfo::read(&dir).unwrap();
    assert_eq!(info.name, "open");
    let config = read_config(&dir).unwrap();
    assert_eq!(config.name, "open");
    let tree = liuhuo_core::scan_tree(&dir).unwrap();
    assert!(!tree.is_empty());
    // schemas 树含 4 个子目录，datas 含 item.json
    let schemas = tree.iter().find(|n| n.name == "schemas").unwrap();
    assert_eq!(schemas.children.len(), 4);
    let datas = tree.iter().find(|n| n.name == "datas").unwrap();
    assert!(datas.children.iter().any(|n| n.name == "item.json"));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn incremental_symbol_table_via_files() {
    let mut s = SymbolTable::new();
    let bean = serde_json::from_str::<liuhuo_core::RawBean>(
        r#"{"name":"Item","fields":[{"name":"q","type":"Quality"}]}"#,
    )
    .unwrap();
    let diags = s.register(&liuhuo_core::RawDef::Bean(bean));
    assert!(diags.iter().any(|d| d.message.contains("Quality")));

    let enum_raw = serde_json::from_str::<liuhuo_core::RawEnum>(
        r#"{"name":"Quality","items":[{"name":"A"}]}"#,
    )
    .unwrap();
    s.register(&liuhuo_core::RawDef::Enum(enum_raw));
    assert!(s.is_ok());
}
