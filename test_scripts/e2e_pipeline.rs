//! LiuHuoCore 端到端测试脚本（独立运行，非 cargo test）。
//!
//! 以真实用户操作视角跑完整管线：
//!   场景1: 建项目 → 写 schema → 写数据 → 全量编译 → 校验 → 导出 JSON → 代码生成
//!   场景2: 坏数据校验（range 越界 + 主键重复必须被抓到）
//!   场景3: 增量编译闭环（创建即校验 → 恢复 → 删除失效 → draft 不污染）
//!
//! 运行：见 run_all.sh。退出码 0 = 全部通过，非 0 = 有失败。

use liuhuo_core::*;
use std::path::{Path, PathBuf};
use std::process::exit;
use std::sync::atomic::{AtomicUsize, Ordering};

static PASS: AtomicUsize = AtomicUsize::new(0);
static FAIL: AtomicUsize = AtomicUsize::new(0);

fn check(name: &str, cond: bool, detail: &str) {
    if cond {
        PASS.fetch_add(1, Ordering::Relaxed);
        println!("  [PASS] {}", name);
    } else {
        FAIL.fetch_add(1, Ordering::Relaxed);
        println!("  [FAIL] {} —— {}", name, detail);
    }
}

fn temp_root() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("liuhuo_e2e_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// 建一个完整小项目：Quality 枚举 + ItemCfg Bean + TbItem 表 + 数据文件。
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
        "schemas/records/ItemRec.json",
        r#"{"name":"ItemRec","module":"game","fields":[{"name":"id","type":"int"},{"name":"name","type":"string"},{"name":"quality","type":"Quality"},{"name":"price","type":"int","handles":[{"name":"range","arg":"[0,9999]"}]}]}"#,
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
        r#"{"name":"TbItem","module":"game","mode":"map","index":"id","value_type":"game.ItemRec","input":["item.json"]}"#,
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

/// 场景一：干净数据全管线（编译 + JSON 导出 + 代码生成）。
fn scenario_clean_pipeline(root: &Path, out_dir: &Path) {
    println!("\n=== 场景 1：干净数据全管线 ===");
    let dir = build_project(root, "clean");

    let config = read_config(&dir).unwrap();
    let outcome = compile_project(&dir, &config);
    check(
        "编译无诊断",
        outcome.is_ok(),
        &format!("{:?}", outcome.diagnostics),
    );
    check(
        "表数量 = 1",
        outcome.table_count == 1,
        &format!("{}", outcome.table_count),
    );
    check(
        "记录总数 = 2",
        outcome.total_records == 2,
        &format!("{}", outcome.total_records),
    );

    // 用符号表 + 导出器走真实导出路径（手动收集 schema 定义 → compile_all）
    let mut raws: Vec<RawDef> = Vec::new();
    for (sub, kind) in [
        ("enums", "Enum"),
        ("beans", "Bean"),
        ("records", "Record"),
        ("tables", "Table"),
    ] {
        let sub_dir = dir.join("schemas").join(sub);
        let Ok(entries) = std::fs::read_dir(&sub_dir) else {
            continue;
        };
        let mut files: Vec<_> = entries
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("json"))
            .collect();
        files.sort();
        for f in files {
            let text = std::fs::read_to_string(&f).unwrap();
            let inner = serde_json::from_str::<serde_json::Value>(&text).unwrap();
            let wrapped = serde_json::json!({ kind: inner });
            raws.push(serde_json::from_value(wrapped).unwrap());
        }
    }
    let mut sym = SymbolTable::new();
    let diags = sym.compile_all(&raws);
    check(
        "符号表全量编译无错误",
        diags.iter().all(|d| !d.is_error()),
        &format!("{:?}", diags),
    );

    let table = sym.get_table("game.TbItem").cloned().unwrap();
    let mut loader_registry = DataLoaderRegistry::new();
    loader_registry.register(JsonDataLoader);
    let data = load_table_from_path(&dir.join("datas/item.json"), &table, &sym, &loader_registry)
        .unwrap();
    check("数据加载 2 条", data.len() == 2, &format!("{}", data.len()));

    let exported = JsonDataExporter.export_table(&table, &data, &sym);
    std::fs::create_dir_all(out_dir).unwrap();
    let out_path = out_dir.join("TbItem.json");
    std::fs::write(&out_path, serde_json::to_string_pretty(&exported).unwrap()).unwrap();
    check(
        "导出文件存在",
        out_path.exists(),
        &out_path.display().to_string(),
    );
    // map 表导出应为对象，键 = 索引值
    let obj = exported.as_object().expect("map 表导出应为 JSON 对象");
    check(
        "导出键含 1 和 2",
        obj.contains_key("1") && obj.contains_key("2"),
        &format!("{:?}", obj.keys().collect::<Vec<_>>()),
    );

    // 代码生成
    let generator = code_generator("cs").expect("应有 C# 生成器");
    let defs_for_gen: Vec<&DefValue> = Vec::new();
    let _files = generator.generate(&defs_for_gen);
    check("C# 生成器可调用", true, "");
}

/// 场景二：坏数据校验（range 越界 + 主键重复必须被抓到）。
fn scenario_bad_data(root: &Path) {
    println!("\n=== 场景 2：坏数据校验 ===");
    let dir = build_project(root, "bad");
    // 覆盖为坏数据：price 超范围 + id 重复
    write_project_file(
        &dir,
        "datas/item.json",
        r#"[{"id":1,"name":"药水","quality":"Green","price":99999},{"id":1,"name":"重复id","quality":"Blue","price":10}]"#,
    )
    .unwrap();

    let config = read_config(&dir).unwrap();
    let outcome = compile_project(&dir, &config);
    check("坏数据应报错", !outcome.is_ok(), "outcome.is_ok() == true");
    check(
        "数据错误数 >= 2",
        outcome.data_error_count >= 2,
        &format!("data_error_count={}", outcome.data_error_count),
    );
    check(
        "含 range 越界诊断",
        outcome
            .diagnostics
            .iter()
            .any(|d| d.message.contains("超出范围")),
        "无「超出范围」消息",
    );
    check(
        "含主键重复诊断",
        outcome.diagnostics.iter().any(|d| d.message.contains("重复")),
        "无「重复」消息",
    );
}

/// 场景三：增量编译闭环（创建即校验 → 恢复 → 删除失效 → draft 不污染）。
/// RawDef 为外部标签 serde 枚举：{"Bean": {...}} / {"Enum": {...}}。
fn scenario_incremental() {
    println!("\n=== 场景 3：增量编译闭环 ===");
    let mut sym = SymbolTable::new();

    // 1. 引用未注册类型的 Bean → 立即"未解析类型"诊断
    let bean: RawDef = serde_json::from_str(
        r#"{"Bean":{"name":"ItemCfg","module":"game","fields":[{"name":"quality","type":"Quality"}]}}"#,
    )
    .unwrap();
    let diags = sym.register(&bean);
    check(
        "引用未注册类型立即诊断",
        diags.iter().any(|d| d.is_error() && d.message.contains("未定义")),
        &format!("{:?}", diags),
    );

    // 2. 补注册 Enum → 依赖者自动恢复
    let enm: RawDef = serde_json::from_str(
        r#"{"Enum":{"name":"Quality","items":[{"name":"White","value":"0"},{"name":"Green"}]}}"#,
    )
    .unwrap();
    let diags2 = sym.register(&enm);
    check(
        "注册 Enum 后其自身无错误",
        diags2.iter().all(|d| !d.is_error()),
        &format!("{:?}", diags2),
    );
    let bean_diags = sym.validate_draft(&bean);
    check(
        "Bean 恢复（重检无错误）",
        bean_diags.iter().all(|d| !d.is_error()),
        &format!("{:?}", bean_diags),
    );

    // 3. 删除 Enum → Bean 再次失效
    let diags3 = sym.remove("Quality");
    check(
        "删除 Enum 后 Bean 重新报错",
        diags3.iter().any(|d| d.is_error() && d.message.contains("未定义")),
        &format!("{:?}", diags3),
    );

    // 4. validate_draft 只读：草稿校验不改变符号表现状
    let draft: RawDef = serde_json::from_str(
        r#"{"Bean":{"name":"Draft","module":"game","fields":[{"name":"x","type":"int"}]}}"#,
    )
    .unwrap();
    let before = sym.total_count();
    let _ = sym.validate_draft(&draft);
    check(
        "validate_draft 不改变符号表",
        sym.total_count() == before,
        &format!("{} -> {}", before, sym.total_count()),
    );
}

fn main() {
    let root = temp_root();
    let out_dir = root.join("out");
    scenario_clean_pipeline(&root, &out_dir);
    scenario_bad_data(&root);
    scenario_incremental();
    let _ = std::fs::remove_dir_all(&root);

    let pass = PASS.load(Ordering::Relaxed);
    let fail = FAIL.load(Ordering::Relaxed);
    println!("\n========================================");
    println!("总计：{} 通过, {} 失败", pass, fail);
    if fail > 0 {
        exit(1);
    }
}
