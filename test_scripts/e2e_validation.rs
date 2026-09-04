//! LiuHuoCore 校验矩阵端到端测试（独立运行，非 cargo test）。
//!
//! K 组（校验矩阵）：覆盖 5 项新校验功能 × 全特性矩阵（普通/flag 枚举、Bean 继承+多态、
//! map(list/one 多表模式)、容器 list/set/map、json 与 .lhd 两种 loader），
//! 每项功能正例 + 反例（精确断言诊断消息与定位），并含增量编译（register/update/remove）验证。
//!
//! 编号接续现有 52 断言（lhd 套件 A-J 组）。
//!
//! 输出：终端 PASS/FAIL 明细 + JSON 报告（test_scripts/report_validation.json，UTF-8）。

use liuhuo_core::*;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::exit;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

static PASS: AtomicUsize = AtomicUsize::new(0);
static FAIL: AtomicUsize = AtomicUsize::new(0);

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

/// 唯一临时根目录（加计数，避免并行/重复运行互相删除）。
static ROOT_COUNTER: AtomicUsize = AtomicUsize::new(0);
fn temp_root() -> PathBuf {
    let n = ROOT_COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "liuhuo_validation_{}_{}_{}",
        std::process::id(),
        n,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// 找诊断：source 含 `src` 且 message 含 `msg`。
fn has_diag(diags: &[Diagnostic], src: &str, msg: &str) -> bool {
    diags
        .iter()
        .any(|d| d.source.as_deref().map(|s| s.contains(src)).unwrap_or(false) && d.message.contains(msg))
}

/// 写枚举（普通 / flag）。
fn write_enum(dir: &Path, name: &str, is_flag: bool, items: &str) {
    let body = if is_flag {
        format!(r#"{{"name":"{}","is_flag":true,"items":[{}]}}"#, name, items)
    } else {
        format!(r#"{{"name":"{}","items":[{}]}}"#, name, items)
    };
    write_project_file(dir, &format!("schemas/enums/{}.json", name), &body).unwrap();
}

/// 构建校验矩阵工程：enum(普通+flag) + Bean 继承(BaseReward→ItemReward) + 多表模式 + 容器 + record。
/// 返回 (项目目录, 资源根目录)。
fn build_matrix(root: &Path, name: &str) -> (PathBuf, PathBuf) {
    create_project(root, name).unwrap();
    let dir = root.join(name);

    // 资源根（path 校验用）
    let res = root.join(format!("{}_res", name));
    std::fs::create_dir_all(&res).unwrap();
    std::fs::write(res.join("icon_a.png"), "png").unwrap();
    std::fs::write(res.join("icon_b.png"), "png").unwrap();

    // 枚举
    write_enum(
        &dir,
        "Quality",
        false,
        r#"{"name":"White","value":"0"},{"name":"Green"},{"name":"Blue"},{"name":"Purple"}"#,
    );
    write_enum(
        &dir,
        "Element",
        true,
        r#"{"name":"None","value":"0"},{"name":"Fire","value":"1"},{"name":"Ice","value":"2"},{"name":"Wind","value":"4"}"#,
    );

    // Record：ItemRec（TbItem 值类型，ref 目标）
    write_project_file(
        &dir,
        "schemas/records/ItemRec.json",
        r#"{"name":"ItemRec","module":"game","fields":[{"name":"id","type":"int"},{"name":"name","type":"string"}]}"#,
    )
    .unwrap();
    // Record：RewardRec（表值类型；句柄化校验声明：nonneg/size/ref/path）
    write_project_file(
        &dir,
        "schemas/records/RewardRec.json",
        r#"{"name":"RewardRec","module":"game","fields":[{"name":"id","type":"int"},{"name":"count","type":"int","handles":[{"name":"nonneg","arg":""}]},{"name":"quality","type":"Quality"},{"name":"elements","type":"set<Element>"},{"name":"tags","type":"list<int>","handles":[{"name":"size","arg":"[1,3]"}]},{"name":"item_refs","type":"list<ref<game.TbItem>>"},{"name":"icon","type":"string","handles":[{"name":"path","arg":""}]}]}"#,
    )
    .unwrap();
    write_project_file(
        &dir,
        "schemas/records/GlobalRec.json",
        r#"{"name":"GlobalRec","module":"game","fields":[{"name":"version","type":"string"},{"name":"max_level","type":"int"}]}"#,
    )
    .unwrap();

    // Record：索引自动唯一（feature 1）
    write_project_file(
        &dir,
        "schemas/records/LootRecord.json",
        r#"{"name":"LootRecord","index":"id","fields":[{"name":"id","type":"int"},{"name":"name","type":"string"}]}"#,
    )
    .unwrap();

    // 表：map(单列) / map(record) / list(联合索引) / one + 多态 map
    write_project_file(
        &dir,
        "schemas/tables/TbItem.json",
        r#"{"name":"TbItem","module":"game","mode":"map","index":"id","value_type":"game.ItemRec","input":["item.json"]}"#,
    )
    .unwrap();
    write_project_file(
        &dir,
        "schemas/tables/TbReward.json",
        r#"{"name":"TbReward","module":"game","mode":"map","index":"id","value_type":"game.RewardRec","input":["reward.json"]}"#,
    )
    .unwrap();
    write_project_file(
        &dir,
        "schemas/tables/TbRewardList.json",
        r#"{"name":"TbRewardList","module":"game","mode":"list","index":"id+count","value_type":"game.RewardRec","input":["reward_list.json"]}"#,
    )
    .unwrap();
    write_project_file(
        &dir,
        "schemas/tables/TbLoot.json",
        r#"{"name":"TbLoot","module":"game","mode":"map","value_type":"LootRecord","input":["loot.lhd"]}"#,
    )
    .unwrap();
    write_project_file(
        &dir,
        "schemas/tables/TbGlobal.json",
        r#"{"name":"TbGlobal","module":"game","mode":"one","value_type":"game.GlobalRec","input":["global.json"]}"#,
    )
    .unwrap();
    write_project_file(
        &dir,
        "schemas/tables/TbDrop.json",
        r#"{"name":"TbDrop","module":"game","mode":"map","index":"id","value_type":"game.RewardRec","input":["drop.json"]}"#,
    )
    .unwrap();

    // 数据：JSON
    write_project_file(
        &dir,
        "datas/item.json",
        r#"[{"id":1,"name":"sword"},{"id":2,"name":"bow"}]"#,
    )
    .unwrap();
    write_project_file(
        &dir,
        "datas/reward.json",
        r#"[{"id":1,"count":5,"quality":"Green","elements":["Fire","Ice"],"tags":[1,2,3],"item_refs":[1,2],"icon":"icon_a.png"},{"id":2,"count":0,"quality":"Blue","elements":["Wind"],"tags":[9],"item_refs":[1],"icon":"icon_b.png"}]"#,
    )
    .unwrap();
    write_project_file(
        &dir,
        "datas/reward_list.json",
        r#"[{"id":1,"count":5,"quality":"Green","elements":["Fire"],"tags":[1],"item_refs":[1],"icon":"icon_a.png"},{"id":1,"count":7,"quality":"Blue","elements":["Ice"],"tags":[2],"item_refs":[2],"icon":"icon_b.png"}]"#,
    )
    .unwrap();
    write_project_file(&dir, "datas/global.json", r#"{"version":"v1","max_level":99}"#).unwrap();
    // 多态数据行：value_type=BaseReward，$type 指定子类 ItemReward（继承字段合并 + nonneg 生效）
    write_project_file(
        &dir,
        "datas/drop.json",
        r#"[{"id":1,"count":5,"quality":"Green","elements":["Fire"],"tags":[1],"item_refs":[1],"icon":"icon_a.png"}]"#,
    )
    .unwrap();

    // 数据：.lhd（record 表，索引自动唯一）
    write_project_file(
        &dir,
        "datas/loot.lhd",
        "## format=lhd\n## version=1\n## table=TbLoot\n## record=LootRecord\n## fields=id;name\n## order=id\n\n{1;\"sword\"}\n{2;\"shield\"}\n",
    )
    .unwrap();

    // 配置：path_root 指向资源根
    let cfg = LiuHuoConfig {
        name: name.to_string(),
        path_root: Some(res.to_string_lossy().to_string()),
        ..Default::default()
    };
    write_config(&dir, &cfg).unwrap();

    (dir, res)
}

/// 重写某张表的数据文件后重新编译，返回 outcome。
fn recompile_with_data(dir: &Path, data_rel: &str, content: &str) -> ProjectCompileOutcome {
    write_project_file(dir, data_rel, content).unwrap();
    let config = read_config(dir).unwrap();
    compile_project(dir, &config)
}

// ============================================================================
// K 组场景
// ============================================================================

fn scenario_matrix_closed_loop(root: &Path) {
    println!("\n=== K. 校验矩阵 ===\n--- K1. 全量编译闭环（json + .lhd 混合，多表模式） ---");
    let (dir, _res) = build_matrix(root, "matrix");
    let config = read_config(&dir).unwrap();
    let o = compile_project(&dir, &config);
    check("K", "矩阵工程编译零诊断", o.is_ok(), &format!("{:#?}", o.diagnostics));
    check("K", "表数量 = 6（map×3 + list + one + drop表）", o.table_count == 6, &format!("{}", o.table_count));
    check("K", "记录总数 = 10（2+2+2+2+1+1）", o.total_records == 10, &format!("{}", o.total_records));
    let cache = read_compile_cache(&dir).unwrap();
    check("K", "编译缓存 ok=true", cache["ok"].as_bool() == Some(true), &cache.to_string());
    // 符号表计数（含 record / enum / bean）
    let mut sym = SymbolTable::new();
    let raws = collect_raws(&dir);
    let diags = sym.compile_all(&raws);
    check("K", "符号表编译零错误", diags.iter().all(|d| !d.is_error()), &format!("{:?}", diags));
    check("K", "枚举=2 / Record=4 / 表=6（Bean=0：表值已全部 Record 化）", sym.enum_count() == 2 && sym.bean_count() == 0 && sym.record_count() == 4 && sym.table_count() == 6, &format!("e{}b{}r{}t{}", sym.enum_count(), sym.bean_count(), sym.record_count(), sym.table_count()));
    // RewardRec 平面字段 = 7（id/count/quality/elements/tags/item_refs/icon）——句柄在字段上而非类型串
    {
        use liuhuo_core::value::DataContext as _;
        let fields = liuhuo_core::DataContext::bean_hierarchy_fields(&sym, "game.RewardRec").expect("RewardRec");
        check("K", "RewardRec 7 字段 + 句柄桥接（nonneg/size/path）",
            fields.len() == 7
                && fields.iter().find(|(n, _)| n == "count").unwrap().1.tags.contains_key("nonneg")
                && fields.iter().find(|(n, _)| n == "tags").unwrap().1.tags.get("size").map(|x| x.as_str()) == Some("[1,3]")
                && fields.iter().find(|(n, _)| n == "icon").unwrap().1.tags.contains_key("path"),
            &format!("{:?}", fields.iter().map(|(n, t)| (n.clone(), t.tags.clone())).collect::<Vec<_>>()));
    }
}

fn scenario_nonneg(root: &Path) {
    println!("\n--- K2. nonneg 非负校验（正例已在闭环，此处反例） ---");
    let (dir, _) = build_matrix(root, "neg_nonneg");
    let o = recompile_with_data(
        &dir,
        "datas/reward.json",
        r#"[{"id":1,"count":-1,"quality":"Green","elements":["Fire"],"tags":[1],"item_refs":[1],"icon":"icon_a.png"},{"id":2,"count":0,"quality":"Blue","elements":["Wind"],"tags":[9],"item_refs":[1],"icon":"icon_b.png"}]"#,
    );
    check("K", "nonneg 反例：count=-1 被捕获", has_diag(&o.diagnostics, "TbReward[行1].count", "值 -1 为负，不满足非负约束"), &format!("{:#?}", o.diagnostics));
    check("K", "nonneg 反例：仅 1 条错误", o.diagnostics.iter().filter(|d| d.is_error()).count() == 1, &format!("{}", o.data_error_count));
    // 多态行中 nonneg 同样生效
    let o2 = recompile_with_data(
        &dir,
        "datas/drop.json",
        r#"[{"id":1,"count":-3,"quality":"Green","elements":["Fire"],"tags":[1],"item_refs":[1],"icon":"icon_a.png"}]"#,
    );
    check("K", "nonneg 反例之二（count=-3 独立行）", has_diag(&o2.diagnostics, "count", "值 -3 为负"), &format!("{:#?}", o2.diagnostics));
}

fn scenario_size(root: &Path) {
    println!("\n--- K3. size 容器大小校验 ---");
    let (dir, _) = build_matrix(root, "neg_size");
    let o = recompile_with_data(
        &dir,
        "datas/reward.json",
        r#"[{"id":1,"count":5,"quality":"Green","elements":["Fire"],"tags":[],"item_refs":[1],"icon":"icon_a.png"},{"id":2,"count":0,"quality":"Blue","elements":["Wind"],"tags":[9],"item_refs":[1],"icon":"icon_b.png"}]"#,
    );
    check("K", "size 反例：空列表被捕获", has_diag(&o.diagnostics, "TbReward[行1].tags", "容器大小 0 超出范围 [1, 3]"), &format!("{:#?}", o.diagnostics));
    // 4 元素超上界
    let o2 = recompile_with_data(
        &dir,
        "datas/reward.json",
        r#"[{"id":1,"count":5,"quality":"Green","elements":["Fire"],"tags":[1,2,3,4],"item_refs":[1],"icon":"icon_a.png"},{"id":2,"count":0,"quality":"Blue","elements":["Wind"],"tags":[9],"item_refs":[1],"icon":"icon_b.png"}]"#,
    );
    check("K", "size 反例：4 元素超上界被捕获", has_diag(&o2.diagnostics, "tags", "容器大小 4 超出范围 [1, 3]"), &format!("{:#?}", o2.diagnostics));
}

fn scenario_ref(root: &Path) {
    println!("\n--- K4. 跨表 ref 校验（含容器内 ref） ---");
    let (dir, _) = build_matrix(root, "neg_ref");
    // 顶层容器内 ref：list<ref<TbItem>> 引用不存在的 999
    let o = recompile_with_data(
        &dir,
        "datas/reward.json",
        r#"[{"id":1,"count":5,"quality":"Green","elements":["Fire"],"tags":[1],"item_refs":[999],"icon":"icon_a.png"},{"id":2,"count":0,"quality":"Blue","elements":["Wind"],"tags":[9],"item_refs":[1],"icon":"icon_b.png"}]"#,
    );
    check("K", "ref 反例：容器内 ref=999 不存在", has_diag(&o.diagnostics, "TbReward[行1].item_refs", "外键值 i999 不存在于表 'game.TbItem'"), &format!("{:#?}", o.diagnostics));
    // 正例已在闭环：item_refs=[1,2] 均存在
    let (dir2, _) = build_matrix(root, "pos_ref");
    let config = read_config(&dir2).unwrap();
    let o2 = compile_project(&dir2, &config);
    check("K", "ref 正例：引用存在的 id 无 ref 诊断", !o2.diagnostics.iter().any(|d| d.message.contains("外键值")), &format!("{:#?}", o2.diagnostics));
}

fn scenario_path(root: &Path) {
    println!("\n--- K5. path 路径存在性校验 ---");
    let (dir, _res) = build_matrix(root, "neg_path");
    // 反例：icon 指向不存在文件
    let o = recompile_with_data(
        &dir,
        "datas/reward.json",
        r#"[{"id":1,"count":5,"quality":"Green","elements":["Fire"],"tags":[1],"item_refs":[1],"icon":"missing_icon.png"},{"id":2,"count":0,"quality":"Blue","elements":["Wind"],"tags":[9],"item_refs":[1],"icon":"icon_b.png"}]"#,
    );
    check("K", "path 反例：不存在的路径被捕获", has_diag(&o.diagnostics, "TbReward[行1].icon", "路径 'missing_icon.png' 不存在"), &format!("{:#?}", o.diagnostics));
    // 正例 + 根目录拼接已在闭环（icon_a.png/icon_b.png 存在）；额外验证：无 path_root 时绝对路径存在通过
    let (dir2, _) = build_matrix(root, "pos_path");
    let cfg = LiuHuoConfig { name: "pos_path".into(), path_root: None, ..Default::default() };
    write_config(&dir2, &cfg).unwrap();
    let abs = root.join("pos_path_res").join("icon_a.png");
    let abs_s = abs.to_string_lossy().to_string();
    let o2 = recompile_with_data(
        &dir2,
        "datas/reward.json",
        &format!(r#"[{{"id":1,"count":5,"quality":"Green","elements":["Fire"],"tags":[1],"item_refs":[1],"icon":"{}"}},{{"id":2,"count":0,"quality":"Blue","elements":["Wind"],"tags":[9],"item_refs":[1],"icon":"{}"}}]"#, abs_s, abs_s),
    );
    check("K", "path 正例：绝对路径存在则通过", !o2.diagnostics.iter().any(|d| d.message.contains("路径")), &format!("{:#?}", o2.diagnostics));
}

fn scenario_record_unique(root: &Path) {
    println!("\n--- K6. record 索引自动唯一（.lhd 加载） ---");
    let (dir, _) = build_matrix(root, "neg_record");
    // .lhd 重复 id
    let o = recompile_with_data(
        &dir,
        "datas/loot.lhd",
        "## format=lhd\n## version=1\n## table=TbLoot\n## record=LootRecord\n## fields=id;name\n## order=id\n\n{1;\"sword\"}\n{1;\"dup\"}\n",
    );
    check("K", "record 反例：.lhd 重复 id 被捕获", has_diag(&o.diagnostics, "TbLoot[行2]", "索引 id 的值重复: i1"), &format!("{:#?}", o.diagnostics));
    // 正例已在闭环（2 条唯一 id）
}

fn scenario_incremental() {
    println!("\n--- K7. 增量编译验证（record 索引引入错误→update 修复） ---");
    let mut sym = SymbolTable::new();

    // 引入错误：record 索引指向不存在字段
    let bad: RawDef = serde_json::from_str(
        r#"{"Record":{"name":"LootRec","index":"missing","fields":[{"name":"id","type":"int"},{"name":"name","type":"string"}]}}"#,
    )
    .unwrap();
    let diags = sym.register(&bad);
    check("K", "增量：索引列不存在立即诊断", diags.iter().any(|d| d.is_error() && d.message.contains("索引列 'missing' 不存在")), &format!("{:?}", diags));

    // 依赖该 record 的表 → 重检
    let table: RawDef = serde_json::from_str(
        r#"{"Table":{"name":"TbLootRec","mode":"list","value_type":"LootRec","input":[]}}"#,
    )
    .unwrap();
    sym.register(&table);

    // update 修复：索引改为 id
    let fixed: RawDef = serde_json::from_str(
        r#"{"Record":{"name":"LootRec","index":"id","fields":[{"name":"id","type":"int"},{"name":"name","type":"string"}]}}"#,
    )
    .unwrap();
    let up_diags = sym.update(&fixed);
    check("K", "增量：update 修复后错误消失", up_diags.iter().all(|d| !d.is_error()), &format!("{:?}", up_diags));
    let rechecked = sym.last_rechecked().to_vec();
    check("K", "增量：只重检依赖者（TbLootRec）", rechecked.contains(&"TbLootRec".to_string()), &format!("rechecked={:?}", rechecked));

    // 无关定义不受影响
    let other: RawDef = serde_json::from_str(
        r#"{"Bean":{"name":"Other","fields":[{"name":"x","type":"int"}]}}"#,
    )
    .unwrap();
    sym.register(&other);
    sym.update(&fixed);
    let rechecked2 = sym.last_rechecked().to_vec();
    check("K", "增量：无关定义不被重检", !rechecked2.contains(&"Other".to_string()), &format!("rechecked={:?}", rechecked2));
}

/// 收集项目全部 schema 定义（外部标签包装），复用现有套件模式。
fn collect_raws(dir: &Path) -> Vec<RawDef> {
    let mut raws = Vec::new();
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
            let inner: serde_json::Value = serde_json::from_str(&text).unwrap();
            let wrapped = serde_json::json!({ kind: inner });
            raws.push(serde_json::from_value(wrapped).unwrap());
        }
    }
    raws
}

fn main() {
    let root = temp_root();
    scenario_matrix_closed_loop(&root);
    scenario_nonneg(&root);
    scenario_size(&root);
    scenario_ref(&root);
    scenario_path(&root);
    scenario_record_unique(&root);
    scenario_incremental();

    let pass = PASS.load(Ordering::Relaxed);
    let fail = FAIL.load(Ordering::Relaxed);
    let report_guard = report();
    let report = report_guard.as_ref().unwrap();
    let mut groups = serde_json::Map::new();
    for (g, cases) in report.iter() {
        let gp = cases.iter().filter(|(_, ok, _)| *ok).count();
        groups.insert(
            g.clone(),
            serde_json::json!({
                "total": cases.len(),
                "passed": gp,
                "failed": cases.len() - gp,
                "cases": cases.iter().map(|(n, ok, d)| serde_json::json!({
                    "name": n, "passed": ok, "detail": if *ok { "" } else { d }
                })).collect::<Vec<_>>(),
            }),
        );
    }
    let doc = serde_json::json!({
        "tool": "liuhuo_core",
        "suite": "校验矩阵端到端测试（K 组）",
        "total": pass + fail,
        "passed": pass,
        "failed": fail,
        "verdict": if fail == 0 { "PASS" } else { "FAIL" },
        "groups": groups,
    });
    let report_path = PathBuf::from("test_scripts/report_validation.json");
    std::fs::write(&report_path, serde_json::to_string_pretty(&doc).unwrap()).unwrap();

    println!("\n========================================");
    println!("校验矩阵总计：{} 通过, {} 失败 —— {}", pass, fail, doc["verdict"]);
    let _ = std::fs::remove_dir_all(&root);
    if fail > 0 {
        exit(1);
    }
}
