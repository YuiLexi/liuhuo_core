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
    assert_eq!(s.record_count(), 2);
    assert_eq!(s.total_count(), 6);
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
fn table_index_resolved_against_record() {
    let (s, _) = build();
    // TbItem 的索引列 id 存在于 game.ItemRec（Record）字段中 → 无诊断即证明校验通过
    assert!(s.is_ok());
    assert_eq!(
        s.kind_of("game.ItemRec"),
        Some(liuhuo_core::DefKind::Record)
    );
}

#[test]
fn bean_value_type_rejected() {
    // 表的值类型只能是 Record：Bean 作 value_type 必须报错
    let defs = common::load_all_project_defs();
    let mut defs: Vec<liuhuo_core::RawDef> = defs;
    // 把 TbItem 的 value_type 篡改为 Bean
    let mut patched = Vec::new();
    for d in defs.drain(..) {
        match d {
            liuhuo_core::RawDef::Table(mut t) => {
                t.value_type = "game.ItemCfg".to_string();
                patched.push(liuhuo_core::RawDef::Table(t));
            }
            other => patched.push(other),
        }
    }
    let mut s = SymbolTable::new();
    let diags = s.compile_all(&patched);
    assert!(
        diags.iter().any(|d| d.is_error() && d.message.contains("只能是 Record")),
        "应报'只能是 Record': {:?}",
        diags
    );
}

#[test]
fn record_field_handles_bridged() {
    // Record 字段句柄桥接到校验标签（type_info.tags），供现有校验器复用
    let defs = common::load_all_project_defs();
    let mut s = SymbolTable::new();
    let _ = s.compile_all(&defs);
    // 通过 DataContext 拿 Record 层级字段的 tags
    use liuhuo_core::value::DataContext as _;
    let fields = s.bean_hierarchy_fields("game.ItemRec").expect("ItemRec");
    let price = fields.iter().find(|(n, _)| n == "price").unwrap();
    assert_eq!(price.1.tags.get("range").map(|s| s.as_str()), Some("[0,9999]"),
        "price 句柄 range 应桥接到 tags: {:?}", price.1.tags);
    let attrs = fields.iter().find(|(n, _)| n == "attrs").unwrap();
    assert_eq!(attrs.1.tags.get("size").map(|s| s.as_str()), Some("10"));
}

#[test]
fn bean_field_handles_rejected() {
    // Bean 字段禁止句柄
    let bean: liuhuo_core::RawDef = serde_json::from_str(
        r#"{"Bean":{"name":"BadCfg","module":"game","fields":[{"name":"x","type":"int","handles":[{"name":"range","arg":"[0,9]"}]}]}}"#,
    )
    .unwrap();
    let mut s = SymbolTable::new();
    let diags = s.register(&bean);
    assert!(
        diags.iter().any(|d| d.is_error() && d.message.contains("不允许句柄")),
        "应报'不允许句柄': {:?}",
        diags
    );
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
