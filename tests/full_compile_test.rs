//! 全量编译集成测试：从 fixtures 加载完整项目定义集 → compile_all。
//!
//! 运行：`cargo test --test full_compile_test`

mod common;

use liuhuo_core::{Diagnostic, SymbolTable};

fn build() -> (SymbolTable, Vec<Diagnostic>) {
    let defs = common::load_all_project_defs();
    let mut s = SymbolTable::new();
    let diags = s.compile_all(&defs);
    (s, diags)
}

#[test]
fn full_project_compiles_clean() {
    let (s, diags) = build();
    assert!(diags.is_empty(), "完整项目应无诊断: {:?}", diags);
    assert!(s.is_ok());
    assert_eq!(s.enum_count(), 1);
    assert_eq!(s.bean_count(), 2);
    assert_eq!(s.table_count(), 1);
    assert_eq!(s.record_count(), 1);
    assert_eq!(s.total_count(), 5);
}

#[test]
fn inheritance_collects_hierarchy_fields() {
    let (s, _) = build();
    let names = s
        .bean_field_names_of("game.WeaponCfg")
        .expect("WeaponCfg 应存在");
    assert_eq!(
        names,
        vec!["id", "name", "quality", "attrs", "atk", "durability"],
        "层级字段应为父类字段 + 自身字段"
    );
}

#[test]
fn table_index_resolved_against_bean() {
    let (s, _) = build();
    // TbItem 的索引列 id 存在于 game.ItemCfg 字段中 → 无诊断即证明校验通过
    assert!(s.is_ok());
}

#[test]
fn module_qualified_refs_resolve() {
    let (s, _) = build();
    // WeaponCfg.parent = "game.ItemCfg"（模块限定）应正确解析
    assert_eq!(
        s.kind_of("game.WeaponCfg"),
        Some(liuhuo_core::DefKind::Bean)
    );
    assert_eq!(s.kind_of("game.ItemCfg"), Some(liuhuo_core::DefKind::Bean));
    assert_eq!(s.kind_of("Quality"), Some(liuhuo_core::DefKind::Enum));
}
