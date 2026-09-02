//! 类型系统独立测试：通过公共 API `parse_type` 验证类型串解析。
//!
//! 运行：`cargo test --test type_parse_test`

use liuhuo_core::{EmptyResolver, MapResolver, TypeKind, parse_type};

#[test]
fn parse_primitives_and_aliases() {
    let r = EmptyResolver;
    assert_eq!(parse_type("int", &r).unwrap().type_name(), "i32");
    assert_eq!(parse_type("long", &r).unwrap().type_name(), "i64");
    assert_eq!(parse_type("byte", &r).unwrap().type_name(), "u8");
    assert_eq!(parse_type("float", &r).unwrap().type_name(), "f32");
    assert_eq!(parse_type("double", &r).unwrap().type_name(), "f64");
    assert_eq!(parse_type("time", &r).unwrap().type_name(), "datetime");
}

#[test]
fn parse_containers() {
    let r = EmptyResolver;
    assert_eq!(
        parse_type("list<int>", &r).unwrap().type_name(),
        "list<i32>"
    );
    assert_eq!(
        parse_type("map<string, list<int>>", &r)
            .unwrap()
            .type_name(),
        "map<string,list<i32>>"
    );
    assert_eq!(
        parse_type("set<string>", &r).unwrap().type_name(),
        "set<string>"
    );
}

#[test]
fn parse_nullable_and_tags() {
    let r = EmptyResolver;
    assert!(parse_type("int?", &r).unwrap().nullable);
    let ti = parse_type("int(range=[1,100])", &r).unwrap();
    assert_eq!(ti.tags.get("range").unwrap(), "[1,100]");
}

#[test]
fn parse_refs_vs_unresolved() {
    let r = MapResolver::new()
        .with_enum("Quality")
        .with_bean("game.ItemCfg");
    assert!(matches!(
        parse_type("Quality", &r).unwrap().kind,
        TypeKind::Enum(_)
    ));
    assert!(matches!(
        parse_type("game.ItemCfg", &r).unwrap().kind,
        TypeKind::Bean(_)
    ));
    match parse_type("Missing", &r).unwrap().kind {
        TypeKind::Unresolved(n) => assert_eq!(n, "Missing"),
        _ => panic!("应为 Unresolved"),
    }
}

#[test]
fn parse_invalid_errs() {
    let r = EmptyResolver;
    assert!(parse_type("", &r).is_err());
    assert!(parse_type("map<int>", &r).is_err()); // map 需 2 参数
}
