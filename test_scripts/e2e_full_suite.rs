//! LiuHuoCore 管线功能闭环综合测试（独立运行，非 cargo test）。
//!
//! 五大测试组：
//!   A. 全量编译       —— compile_project（schema 加载→符号表→数据→校验→缓存）
//!   B. 增量编译       —— register/update/remove/validate_draft + last_rechecked 增量正确性
//!   C. 数据校验       —— range 越界 / 主键重复 / 枚举非法值 / 类型不匹配 / 空值违例 / one 表多条
//!   D. 代码生成       —— cs / ts / rust 三目标，内容含枚举、继承 Bean、类型映射
//!   E. 数据生成(导出) —— map/list/one 三模式导出 + 公式物化（apply_formula / compute_columns）
//!
//! 输出：终端 PASS/FAIL 明细 + JSON 报告（test_scripts/report.json，UTF-8）。

use liuhuo_core::*;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::exit;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

static PASS: AtomicUsize = AtomicUsize::new(0);
static FAIL: AtomicUsize = AtomicUsize::new(0);

/// 报告数据：组名 -> (用例名, 通过与否, 失败详情) 列表。
type ReportMap = BTreeMap<String, Vec<(String, bool, String)>>;
static REPORT: Mutex<Option<ReportMap>> = Mutex::new(None);

fn report() -> std::sync::MutexGuard<'static, Option<ReportMap>> {
    REPORT.lock().unwrap_or_else(|e| e.into_inner())
}

fn check(group: &str, name: &str, cond: bool, detail: &str) {
    if cond {
        PASS.fetch_add(1, Ordering::Relaxed);
        println!("  [PASS] {}", name);
    } else {
        FAIL.fetch_add(1, Ordering::Relaxed);
        println!("  [FAIL] {} —— {}", name, detail);
    }
    let mut rep = report();
    let rep = rep.get_or_insert_with(BTreeMap::new);
    rep.entry(group.to_string())
        .or_default()
        .push((name.to_string(), cond, detail.to_string()));
}

fn temp_root() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("liuhuo_e2e_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// 从项目目录收集全部 schema 定义（外部标签包装）。
fn collect_raws(dir: &Path) -> Vec<RawDef> {
    let mut raws = Vec::new();
    for (sub, kind) in [
        ("enums", "Enum"),
        ("beans", "Bean"),
        ("records", "Record"),
        ("tables", "Table"),
    ] {
        let sub_dir = dir.join("schemas").join(sub);
        let Ok(entries) = std::fs::read_dir(&sub_dir) else { continue };
        let mut files: Vec<_> = entries
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("json"))
            .collect();
        files.sort();
        for f in files {
            let text = std::fs::read_to_string(&f).unwrap();
            let inner: serde_json::Value = serde_json::from_str(&text).unwrap();
            let wrapped = serde_json::json!({ kind: inner });
            raws.push(serde_json::from_value(wrapped).unwrap());
        }
    }
    raws
}

/// 标准测试项目：枚举 + 继承 Bean + map/list/one 三表 + 文本数据。
fn build_rich_project(root: &Path, name: &str) -> PathBuf {
    create_project(root, name).unwrap();
    let dir = root.join(name);

    write_project_file(&dir, "schemas/enums/Quality.json",
        r#"{"name":"Quality","comment":"品质","items":[{"name":"White","value":"0"},{"name":"Green"},{"name":"Blue"},{"name":"Purple"}]}"#).unwrap();
    write_project_file(&dir, "schemas/enums/Element.json",
        r#"{"name":"Element","comment":"元素","is_flag":true,"items":[{"name":"None","value":"0"},{"name":"Fire","value":"1"},{"name":"Ice","value":"2"},{"name":"Both","value":"3"}]}"#).unwrap();
    // 继承体系：BaseItem(id,name) <- EquipCfg(继承+自有字段)
    write_project_file(&dir, "schemas/beans/BaseItem.json",
        r#"{"name":"BaseItem","module":"game","comment":"道具基类","fields":[{"name":"id","type":"int"},{"name":"name","type":"string"}]}"#).unwrap();
    write_project_file(&dir, "schemas/beans/EquipCfg.json",
        r#"{"name":"EquipCfg","module":"game","comment":"装备","parent":"game.BaseItem","fields":[{"name":"quality","type":"Quality"},{"name":"atk","type":"int"},{"name":"price","type":"int(range=[0,9999])"},{"name":"tags","type":"list<string>"},{"name":"attr","type":"map<string,int>"}]}"#).unwrap();
    // map 表
    write_project_file(&dir, "schemas/tables/TbEquip.json",
        r#"{"name":"TbEquip","module":"game","comment":"装备表","mode":"map","index":"id","value_type":"game.EquipCfg","input":["equip.json"]}"#).unwrap();
    // list 表（联合索引 id+quality 唯一）
    write_project_file(&dir, "schemas/tables/TbEquipList.json",
        r#"{"name":"TbEquipList","module":"game","comment":"装备列表表","mode":"list","index":"id+quality","value_type":"game.EquipCfg","input":["equip_list.json"]}"#).unwrap();
    // one 表（单例，用独立小 Bean）
    write_project_file(&dir, "schemas/beans/GlobalCfg.json",
        r#"{"name":"GlobalCfg","module":"game","comment":"全局配置","fields":[{"name":"version","type":"string"},{"name":"max_level","type":"int"}]}"#).unwrap();
    write_project_file(&dir, "schemas/tables/TbGlobal.json",
        r#"{"name":"TbGlobal","module":"game","comment":"全局配置表","mode":"one","value_type":"game.GlobalCfg","input":["global.txt"]}"#).unwrap();

    // JSON 数据（map 表）
    write_project_file(&dir, "datas/equip.json",
        r#"[
          {"id":1,"name":"铁剑","quality":"Green","atk":10,"price":100,"tags":["武器","金属"],"attr":{"锐利":5,"重量":3}},
          {"id":2,"name":"寒冰弓","quality":"Blue","atk":22,"price":800,"tags":["武器","冰霜"],"attr":{"冰伤":12}},
          {"id":3,"name":"紫金冠","quality":"Purple","atk":0,"price":4200,"tags":["头饰"],"attr":{}}
        ]"#).unwrap();
    // JSON 数据（list 表）
    write_project_file(&dir, "datas/equip_list.json",
        r#"[{"id":1,"quality":"Green","name":"铁剑","atk":10,"price":100,"tags":[],"attr":{}},{"id":2,"quality":"Blue","name":"寒冰弓","atk":22,"price":800,"tags":[],"attr":{}}]"#).unwrap();
    // 文本格式数据（one 表）
    write_project_file(&dir, "datas/global.txt",
        "#version:0.1\n\"v1.0.3\"|120\n").unwrap();
    dir
}

// ============================================================================
// A. 全量编译
// ============================================================================

fn scenario_full_compile(root: &Path) {
    println!("\n=== A. 全量编译（compile_project） ===");
    let dir = build_rich_project(root, "full");
    let config = read_config(&dir).unwrap();
    let outcome = compile_project(&dir, &config);

    check("A", "编译零诊断", outcome.is_ok(), &format!("{:#?}", outcome.diagnostics));
    check("A", "表数量 = 3（map+list+one）", outcome.table_count == 3,
        &format!("table_count={}", outcome.table_count));
    check("A", "记录总数 = 6（3+2+1）", outcome.total_records == 6,
        &format!("total_records={}", outcome.total_records));

    // 编译缓存
    let cache = read_compile_cache(&dir).unwrap();
    check("A", "编译缓存 ok=true", cache["ok"].as_bool() == Some(true), &cache.to_string());
    check("A", "缓存记录数一致", cache["totalRecords"].as_i64() == Some(6), &cache.to_string());

    // 符号表计数
    let mut sym = SymbolTable::new();
    let diags = sym.compile_all(&collect_raws(&dir));
    check("A", "符号表编译零错误", diags.iter().all(|d| !d.is_error()), &format!("{:?}", diags));
    check("A", "枚举数 = 2", sym.enum_count() == 2, &format!("{}", sym.enum_count()));
    check("A", "Bean 数 = 3", sym.bean_count() == 3, &format!("{}", sym.bean_count()));
    // 继承层级字段：EquipCfg 应含父类 id/name + 自身 5 字段 = 7
    let fields = sym.bean_field_names_of("game.EquipCfg").unwrap();
    check("A", "继承层级字段 = 7（父2+自5）", fields.len() == 7, &format!("{:?}", fields));
    check("A", "层级字段含父类 id", fields.first().map(|s| s.as_str()) == Some("id"), &format!("{:?}", fields));
}

// ============================================================================
// B. 增量编译
// ============================================================================

fn scenario_incremental() {
    println!("\n=== B. 增量编译（register/update/remove） ===");
    let mut sym = SymbolTable::new();

    // B1 创建即校验：引用未定义类型
    let bean: RawDef = serde_json::from_str(
        r#"{"Bean":{"name":"ItemCfg","module":"game","fields":[{"name":"quality","type":"Quality"}]}}"#).unwrap();
    let diags = sym.register(&bean);
    check("B", "创建即校验：未定义类型立即诊断",
        diags.iter().any(|d| d.is_error() && d.message.contains("未定义")), &format!("{:?}", diags));

    // B2 补注册 Enum → 依赖者恢复
    let enm: RawDef = serde_json::from_str(
        r#"{"Enum":{"name":"Quality","items":[{"name":"White","value":"0"},{"name":"Green"}]}}"#).unwrap();
    sym.register(&enm);
    let bean_diags = sym.validate_draft(&bean);
    check("B", "恢复：Enum 注册后 Bean 无错误",
        bean_diags.iter().all(|d| !d.is_error()), &format!("{:?}", bean_diags));

    // B3 增量重检范围：改 Quality 后只重检 Quality + 依赖者 ItemCfg，无关定义不动
    let other: RawDef = serde_json::from_str(
        r#"{"Bean":{"name":"OtherCfg","module":"game","fields":[{"name":"x","type":"int"}]}}"#).unwrap();
    sym.register(&other);
    let enm2: RawDef = serde_json::from_str(
        r#"{"Enum":{"name":"Quality","items":[{"name":"White","value":"0"},{"name":"Green"},{"name":"Gold"}]}}"#).unwrap();
    sym.update(&enm2);
    let rechecked = sym.last_rechecked().to_vec();
    check("B", "增量重检：改动只波及 Quality+ItemCfg",
        rechecked.contains(&"game.ItemCfg".to_string()) && !rechecked.contains(&"game.OtherCfg".to_string()),
        &format!("rechecked={:?}", rechecked));

    // B4 删除传播
    let del_diags = sym.remove("Quality");
    check("B", "删除传播：Bean 立即报未定义类型",
        del_diags.iter().any(|d| d.is_error() && d.message.contains("未定义")), &format!("{:?}", del_diags));
    // 再注册恢复
    sym.register(&enm);
    check("B", "删除后重建：依赖者再次恢复",
        sym.validate_draft(&bean).iter().all(|d| !d.is_error()), "重建后 Bean 仍有错误");

    // B5 validate_draft 只读
    let before = sym.total_count();
    let draft: RawDef = serde_json::from_str(
        r#"{"Bean":{"name":"Ghost","module":"game","fields":[{"name":"x","type":"int"}]}}"#).unwrap();
    let _ = sym.validate_draft(&draft);
    check("B", "validate_draft 不污染符号表", sym.total_count() == before,
        &format!("{} -> {}", before, sym.total_count()));

    // B6 update 未注册定义应报错
    let unknown: RawDef = serde_json::from_str(
        r#"{"Bean":{"name":"NotRegistered","module":"game","fields":[]}}"#).unwrap();
    let udiags = sym.update(&unknown);
    check("B", "update 未注册定义报错", udiags.iter().any(|d| d.is_error()), &format!("{:?}", udiags));
}

// ============================================================================
// C. 数据校验
// ============================================================================

fn scenario_validation(root: &Path) {
    println!("\n=== C. 数据校验 ===");

    // C1 range 越界 + 主键重复
    let dir = build_rich_project(root, "val1");
    write_project_file(&dir, "datas/equip.json",
        r#"[{"id":1,"name":"甲","quality":"Green","atk":10,"price":99999,"tags":[],"attr":{}},{"id":1,"name":"重复","quality":"Blue","atk":1,"price":10,"tags":[],"attr":{}}]"#).unwrap();
    let config = read_config(&dir).unwrap();
    let o = compile_project(&dir, &config);
    check("C", "range 越界被捕获", o.diagnostics.iter().any(|d| d.message.contains("超出范围")), &format!("{:?}", o.diagnostics));
    check("C", "map 主键重复被捕获", o.diagnostics.iter().any(|d| d.message.contains("重复")), &format!("{:?}", o.diagnostics));
    check("C", "数据错误计数 >= 2", o.data_error_count >= 2, &format!("{}", o.data_error_count));

    // C2 枚举非法值 + 类型不匹配
    let dir = build_rich_project(root, "val2");
    write_project_file(&dir, "datas/equip.json",
        r#"[{"id":1,"name":"甲","quality":"Rainbow","atk":10,"price":10,"tags":[],"attr":{}},{"id":2,"name":"乙","quality":"Green","atk":"不是数字","price":10,"tags":[],"attr":{}}]"#).unwrap();
    let config = read_config(&dir).unwrap();
    let o = compile_project(&dir, &config);
    check("C", "枚举非法值被捕获", o.diagnostics.iter().any(|d| d.message.contains("Rainbow") || d.message.contains("枚举")), &format!("{:?}", o.diagnostics));
    // 注：JSON 解析错误为文件级 fail-fast（一次报一条），类型不匹配单独一项目验证
    let dir = build_rich_project(root, "val2b");
    write_project_file(&dir, "datas/equip.json",
        r#"[{"id":1,"name":"乙","quality":"Green","atk":"不是数字","price":10,"tags":[],"attr":{}}]"#).unwrap();
    let config = read_config(&dir).unwrap();
    let o = compile_project(&dir, &config);
    check("C", "类型不匹配被捕获", o.diagnostics.iter().any(|d| d.message.contains("不是数字") || d.message.contains("类型")), &format!("{:?}", o.diagnostics));

    // C3 one 表多条记录
    let dir = build_rich_project(root, "val3");
    write_project_file(&dir, "datas/global.txt",
        "#version:0.1\n\"v1\"|99\n\"v2\"|100\n").unwrap();
    let config = read_config(&dir).unwrap();
    let o = compile_project(&dir, &config);
    check("C", "one 表多条记录被捕获",
        o.diagnostics.iter().any(|d| d.message.contains("仅有 1 条") || d.message.contains("一条") || d.message.contains("记录数")),
        &format!("{:?}", o.diagnostics));

    // C4 list 表联合索引唯一性
    let dir = build_rich_project(root, "val4");
    write_project_file(&dir, "datas/equip_list.json",
        r#"[{"id":1,"quality":"Green","name":"a","atk":1,"price":1,"tags":[],"attr":{}},{"id":1,"quality":"Green","name":"b","atk":2,"price":2,"tags":[],"attr":{}}]"#).unwrap();
    let config = read_config(&dir).unwrap();
    let o = compile_project(&dir, &config);
    check("C", "list 联合索引重复被捕获", o.diagnostics.iter().any(|d| d.message.contains("重复")), &format!("{:?}", o.diagnostics));

    // C5 文本格式正常加载（one 表恰好 1 条不报错）
    let dir = build_rich_project(root, "val5");
    let config = read_config(&dir).unwrap();
    let o = compile_project(&dir, &config);
    check("C", "文本格式数据 + one 表单条通过", o.is_ok(), &format!("{:?}", o.diagnostics));
}

// ============================================================================
// D. 代码生成
// ============================================================================

fn scenario_codegen(root: &Path) {
    println!("\n=== D. 代码生成（cs / ts / rust） ===");
    let dir = build_rich_project(root, "codegen");
    let mut sym = SymbolTable::new();
    let diags = sym.compile_all(&collect_raws(&dir));
    assert!(diags.iter().all(|d| !d.is_error()));

    // 从符号表取全部 DefValue（通过 bean_field_names_of/table_names 已知集合，
    // 这里用 compile_all 后的 defs——通过再次全量编译拿 Def 值列表）
    let mut defs: Vec<DefValue> = Vec::new();
    let raws = collect_raws(&dir);
    let mut sym2 = SymbolTable::new();
    let _ = sym2.compile_all(&raws);
    // DefValue 需要从符号表导出——用 update 触发？直接重建：逐个 register 并收集
    // 更简单：compile_all 已建 defs 缓存，通过 validate_draft 拿不到 Def 值；
    // 用 codegen 的入口：generator.generate(&[&DefValue])。DefValue 从 compile_enum 等拿。
    for raw in &raws {
        match raw {
            RawDef::Enum(r) => { let (d, _, _) = defs_compile_enum(r); defs.push(DefValue::Enum(d)); }
            RawDef::Bean(r) => { let (d, _, _) = defs_compile_bean(r, &sym2); defs.push(DefValue::Bean(d)); }
            _ => {}
        }
    }
    let def_refs: Vec<&DefValue> = defs.iter().collect();
    check("D", "定义收集 = 5（2枚举+3Bean）", def_refs.len() == 5, &format!("{}", def_refs.len()));

    for lang in ["cs", "ts", "rust"] {
        let generator = code_generator(lang).unwrap_or_else(|| panic!("{} 生成器缺失", lang));
        let files = generator.generate(&def_refs);
        let (fname, content) = &files[0];
        let out_path = root.join("codegen").join(lang).join(fname);
        std::fs::create_dir_all(out_path.parent().unwrap()).unwrap();
        std::fs::write(&out_path, content).unwrap();

        check("D", &format!("{} 文件生成（{}）", lang.to_uppercase(), fname), out_path.exists(), "文件未写出");
        check("D", &format!("{} 含枚举 Quality", lang.to_uppercase()), content.contains("Quality"), "缺 Quality");
        check("D", &format!("{} 含继承 EquipCfg", lang.to_uppercase()), content.contains("EquipCfg"), "缺 EquipCfg");
        check("D", &format!("{} 含层级字段 price", lang.to_uppercase()), content.contains("price") || content.contains("Price"), "缺 price");
    }
    // cs 专属：继承语法
    let cs = std::fs::read_to_string(root.join("codegen/cs/Config.cs")).unwrap();
    check("D", "C# 继承语法（含 BaseItem 基类）", cs.contains("BaseItem") && cs.contains("EquipCfg"), "缺基类");
    // ts 专属：interface
    let ts = std::fs::read_to_string(root.join("codegen/ts/config.ts")).unwrap();
    check("D", "TS interface 生成", ts.contains("export interface"), "缺 interface");
    // rust 专属：struct
    let rs = std::fs::read_to_string(root.join("codegen/rust/config.rs")).unwrap();
    check("D", "Rust struct 生成", rs.contains("pub struct"), "缺 struct");
}

// ============================================================================
// E. 数据生成（导出 + 公式）
// ============================================================================

fn scenario_export(root: &Path) {
    println!("\n=== E. 数据生成（导出 + 公式物化） ===");
    let dir = build_rich_project(root, "export");
    let mut sym = SymbolTable::new();
    let _ = sym.compile_all(&collect_raws(&dir));

    let out_dir = root.join("export_out");
    std::fs::create_dir_all(&out_dir).unwrap();
    let mut registry = DataLoaderRegistry::new();
    registry.register(JsonDataLoader);
    registry.register(TextDataLoader);

    // E1 map 表导出：对象，键 = id
    let tb_equip = sym.get_table("game.TbEquip").cloned().unwrap();
    let equip_data = load_table_from_path(&dir.join("datas/equip.json"), &tb_equip, &sym, &registry).unwrap();
    let equip_json = JsonDataExporter.export_table(&tb_equip, &equip_data, &sym);
    std::fs::write(out_dir.join("TbEquip.json"), serde_json::to_string_pretty(&equip_json).unwrap()).unwrap();
    let obj = equip_json.as_object().expect("map 表导出应为对象");
    check("E", "map 导出 = 对象且键为 1/2/3",
        obj.len() == 3 && obj.contains_key("1") && obj.contains_key("3"),
        &format!("{:?}", obj.keys().collect::<Vec<_>>()));
    check("E", "map 导出记录字段完整（name/quality/attr）",
        obj["1"].get("name").is_some() && obj["1"].get("quality").is_some() && obj["1"].get("attr").is_some(),
        &obj["1"].to_string());
    check("E", "枚举字段导出为数值（Green=1）", obj["1"]["quality"].is_i64(), &obj["1"]["quality"].to_string());

    // E2 list 表导出：数组，保留行序
    let tb_list = sym.get_table("game.TbEquipList").cloned().unwrap();
    let list_data = load_table_from_path(&dir.join("datas/equip_list.json"), &tb_list, &sym, &registry).unwrap();
    let list_json = JsonDataExporter.export_table(&tb_list, &list_data, &sym);
    std::fs::write(out_dir.join("TbEquipList.json"), serde_json::to_string_pretty(&list_json).unwrap()).unwrap();
    let arr = list_json.as_array().expect("list 表导出应为数组");
    check("E", "list 导出 = 数组且 2 条", arr.len() == 2, &format!("{}", arr.len()));
    check("E", "list 导出保留行序", arr[0]["name"].as_str() == Some("铁剑"), &arr[0].to_string());

    // E3 one 表导出：单对象（文本格式数据源）
    let tb_one = sym.get_table("game.TbGlobal").cloned().unwrap();
    let one_data = load_table_from_path(&dir.join("datas/global.txt"), &tb_one, &sym, &registry).unwrap();
    let one_json = JsonDataExporter.export_table(&tb_one, &one_data, &sym);
    std::fs::write(out_dir.join("TbGlobal.json"), serde_json::to_string_pretty(&one_json).unwrap()).unwrap();
    check("E", "one 导出 = 单对象", one_json.get("version").is_some(), &one_json.to_string());
    check("E", "文本格式数据正确解码", one_json["version"].as_str() == Some("v1.0.3") && one_json["max_level"].as_i64() == Some(120), &one_json.to_string());

    // E4 公式：apply_formula 批量填充
    let mut data2 = equip_data.clone();
    let fields = TypeResolver::bean_hierarchy_fields(&sym, "game.EquipCfg").unwrap();
    let updated = apply_formula(&mut data2, &fields, "atk", "price * 2 + 1").unwrap();
    check("E", "apply_formula 更新 3 行", updated == 3, &format!("{}", updated));
    // 验证写回值：id=1 price=100 → 201
    let pos_atk = fields.iter().position(|(n, _)| n == "atk").unwrap();
    let pos_id = fields.iter().position(|(n, _)| n == "id").unwrap();
    let row1 = data2.records.iter().find(|r| matches!(r.data.get(pos_id), Some(DType::Int(1)))).unwrap();
    check("E", "公式计算正确（100*2+1=201）",
        matches!(row1.data.get(pos_atk), Some(DType::Int(201))), &format!("{:?}", row1.data.get(pos_atk)));

    // E5 公式：compute_columns 物化
    let cols = vec![ComputedColumn {
        field: "atk".to_string(),
        type_str: "int".to_string(),
        expr: "atk + 100".to_string(),
    }];
    let record0 = &equip_data.records[0];
    let computed = compute_columns(&record0.data, &fields, &cols).unwrap();
    check("E", "compute_columns 物化（atk+100）",
        computed.len() == 1 && matches!(&computed[0].1, DType::Int(v) if *v == 110),
        &format!("{:?}", computed));

    // E6 全部产物落盘
    let files: Vec<_> = std::fs::read_dir(&out_dir).unwrap().filter_map(|e| e.ok()).map(|e| e.path()).collect();
    check("E", "导出产物 3 个文件", files.len() == 3, &format!("{:?}", files.iter().map(|f| f.file_name()).collect::<Vec<_>>()));
}

// lib 内部 compile 函数的包装（通过公开 API 重建 Def 值供代码生成）
fn defs_compile_enum(r: &liuhuo_core::RawEnum) -> (DefEnum, Vec<String>, Vec<Diagnostic>) {
    liuhuo_core::compile_enum(r)
}
fn defs_compile_bean(r: &liuhuo_core::RawBean, ctx: &dyn TypeResolver) -> (DefBean, Vec<String>, Vec<Diagnostic>) {
    liuhuo_core::compile_bean(r, ctx)
}

fn main() {
    let root = temp_root();
    scenario_full_compile(&root);
    scenario_incremental();
    scenario_validation(&root);
    scenario_codegen(&root);
    scenario_export(&root);

    // JSON 报告
    let pass = PASS.load(Ordering::Relaxed);
    let fail = FAIL.load(Ordering::Relaxed);
    let report_guard = report();
    let report = report_guard.as_ref().unwrap();
    let mut groups = serde_json::Map::new();
    for (g, cases) in report.iter() {
        let gp = cases.iter().filter(|(_, ok, _)| *ok).count();
        groups.insert(g.clone(), serde_json::json!({
            "total": cases.len(),
            "passed": gp,
            "failed": cases.len() - gp,
            "cases": cases.iter().map(|(n, ok, d)| serde_json::json!({
                "name": n, "passed": ok, "detail": if *ok { "" } else { d }
            })).collect::<Vec<_>>(),
        }));
    }
    let doc = serde_json::json!({
        "tool": "liuhuo_core",
        "suite": "管线功能闭环综合测试",
        "total": pass + fail,
        "passed": pass,
        "failed": fail,
        "verdict": if fail == 0 { "PASS" } else { "FAIL" },
        "groups": groups,
    });
    let report_path = PathBuf::from("test_scripts/report.json");
    std::fs::write(&report_path, serde_json::to_string_pretty(&doc).unwrap()).unwrap();

    println!("\n========================================");
    println!("总计：{} 通过, {} 失败 —— {}", pass, fail, doc["verdict"]);
    println!("报告：{}", report_path.canonicalize().unwrap().display());
    let _ = std::fs::remove_dir_all(&root);
    if fail > 0 {
        exit(1);
    }
}
