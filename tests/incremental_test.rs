//! 增量编译集成测试：从 fixtures 单文件加载，验证 register / update / remove / validate_draft。
//!
//! 运行单个测试：`cargo test --test incremental_test <测试名>`

mod common;

use liuhuo_core::{RawBean, RawDef, RawEnum, RawEnumItem, RawField, SymbolTable};
use std::fs;

/// 从 fixtures 加载 Quality 枚举并追加一个枚举项，返回新的 RawDef（模拟"编辑"）。
fn quality_with_extra_item() -> RawDef {
    let path = common::fixtures_dir().join("enums/Quality.json");
    let s = fs::read_to_string(path).unwrap();
    let mut q: RawEnum = serde_json::from_str(&s).unwrap();
    q.items.push(RawEnumItem {
        name: "Rainbow".into(),
        ..Default::default()
    });
    RawDef::Enum(q)
}

#[test]
fn create_bean_before_enum_reports_unresolved() {
    let mut s = SymbolTable::new();
    // ItemCfg 引用 Quality，但 Quality 尚未注册 → 未解析
    let diags = s.register(&common::load_raw("beans/ItemCfg.json"));
    assert!(
        diags.iter().any(|d| d.message.contains("Quality")),
        "应报未解析 Quality: {:?}",
        diags
    );
    assert!(!s.is_ok());
}

#[test]
fn register_enum_then_bean_is_clean() {
    let mut s = SymbolTable::new();
    s.register(&common::load_raw("enums/Quality.json"));
    let diags = s.register(&common::load_raw("beans/ItemCfg.json"));
    assert!(diags.is_empty(), "先注册枚举应无诊断: {:?}", diags);
    assert!(s.is_ok());
}

#[test]
fn update_enum_propagates_to_dependents_only() {
    let mut s = SymbolTable::new();
    s.register(&common::load_raw("enums/Quality.json"));
    s.register(&common::load_raw("beans/ItemCfg.json")); // 依赖 Quality
    s.register(&common::load_raw("records/RowData.json")); // 无关定义
    assert!(s.is_ok());

    s.update(&quality_with_extra_item());

    let rechecked = s.last_rechecked();
    assert!(
        rechecked.iter().any(|n| n == "game.ItemCfg"),
        "ItemCfg 应被重检: {:?}",
        rechecked
    );
    assert!(
        !rechecked.iter().any(|n| n == "game.RowData"),
        "RowData 不应被重检: {:?}",
        rechecked
    );
    assert!(s.is_ok());
}

#[test]
fn remove_enum_invalidates_bean() {
    let mut s = SymbolTable::new();
    s.register(&common::load_raw("enums/Quality.json"));
    s.register(&common::load_raw("beans/ItemCfg.json"));
    assert!(s.is_ok());

    let diags = s.remove("Quality");
    assert!(diags.iter().any(|d| d.message.contains("Quality")));
    assert!(!s.is_ok());
    assert_eq!(s.total_count(), 1); // 只剩 ItemCfg
}

#[test]
fn validate_draft_does_not_pollute() {
    let mut s = SymbolTable::new();
    s.register(&common::load_raw("enums/Quality.json"));
    let before = s.total_count();

    // 一个引用不存在类型的草稿
    let bad = RawDef::Bean(RawBean {
        name: "Draft".into(),
        fields: vec![RawField {
            name: "q".into(),
            r#type: "Nope".into(),
            ..Default::default()
        }],
        ..Default::default()
    });
    let diags = s.validate_draft(&bad);
    assert!(diags.iter().any(|d| d.message.contains("Nope")));
    assert_eq!(s.total_count(), before, "validate_draft 不应改变符号表");
    assert!(!s.has("Draft"));
}

#[test]
fn remove_then_register_recovers() {
    let mut s = SymbolTable::new();
    s.register(&common::load_raw("enums/Quality.json"));
    s.register(&common::load_raw("beans/ItemCfg.json"));
    s.remove("Quality");
    assert!(!s.is_ok());

    // 重新注册 Quality → 依赖者重检后错误消失
    s.register(&common::load_raw("enums/Quality.json"));
    assert!(s.is_ok());
}
