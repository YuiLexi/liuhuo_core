//! `.lhd` 内置数据格式端到端测试（test 分支，独立运行非 cargo test）。
//!
//! 八个测试组（对齐设计文档 docs/数据格式-lhd.md）：
//!   A. 头部指令        —— 解析/自定义元数据/未知指令容忍/缺 fields 报错
//!   B. 行加载          —— bean 字面量行/嵌套容器/枚举裸名/类型指导
//!   C. 停用行          —— : 前缀/内容仍可解析/不进数据/保存原位保留
//!   D. 数据标签        —— @tag 多标签/键值标签/引号内 @tag 不误判
//!   E. 注释            —— 整行 // / 行尾 // / 字符串内 // 不受影响
//!   F. 多态行          —— @type 标记/无标记默认/行首 @type 缺括号报错
//!   G. schema 漂移     —— fields 错位=硬错误/指纹不匹配=警告/逐行错误收集
//!   H. 确定性保存+性能  —— 幂等往返/主键乱序重排/list 顺序保留/10万行加载性能
//!   I. 严格语法          —— k=v only/引号规则/空格过滤
//!
//! 输出：终端 PASS/FAIL 明细 + JSON 报告（test_scripts/report_lhd.json）。

use liuhuo_core::defs::{DefTable, DefKind, RawDef, TableIndex};
use liuhuo_core::types::{TypeInfo, TypeKind};
use liuhuo_core::value::{DataContext, DType};
use liuhuo_core::{
    LhdDataLoader, SymbolTable, compile_enum, load_lhd_from_str, load_table_from_path, save_lhd,
    schema_fingerprint,
};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::exit;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::Instant;

static PASS: AtomicUsize = AtomicUsize::new(0);
static FAIL: AtomicUsize = AtomicUsize::new(0);
type ReportMap = BTreeMap<String, Vec<(String, bool, String)>>;
static REPORT: Mutex<Option<ReportMap>> = Mutex::new(None);

fn check(group: &str, name: &str, cond: bool, detail: &str) {
    if cond {
        PASS.fetch_add(1, Ordering::Relaxed);
        println!("  [PASS] {}", name);
    } else {
        FAIL.fetch_add(1, Ordering::Relaxed);
        println!("  [FAIL] {} —— {}", name, detail);
    }
    let mut rep = REPORT.lock().unwrap_or_else(|e| e.into_inner());
    let rep = rep.get_or_insert_with(BTreeMap::new);
    rep.entry(group.to_string())
        .or_default()
        .push((name.to_string(), cond, detail.to_string()));
}

fn temp_root() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("liuhuo_lhd_e2e_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

// ============================================================================
// 测试基建：真实符号表（Quality 枚举 + EquipCfg bean + 继承 ItemBase）
// ============================================================================

fn build_symtab() -> SymbolTable {
    let mut sym = SymbolTable::new();
    let enm: RawDef = serde_json::from_str(
        r#"{"Enum":{"name":"Quality","items":[{"name":"White","value":"0"},{"name":"Green"},{"name":"Blue"},{"name":"Purple"}]}}"#,
    )
    .unwrap();
    let _ = sym.register(&enm);
    let base: RawDef = serde_json::from_str(
        r#"{"Bean":{"name":"ItemBase","module":"game","fields":[{"name":"id","type":"int"},{"name":"name","type":"string"}]}}"#,
    )
    .unwrap();
    let _ = sym.register(&base);
    let equip: RawDef = serde_json::from_str(
        r#"{"Bean":{"name":"EquipCfg","module":"game","parent":"game.ItemBase","fields":[{"name":"quality","type":"Quality"},{"name":"atk","type":"int"},{"name":"tags","type":"list<string>"},{"name":"attr","type":"map<string,int>"}]}}"#,
    )
    .unwrap();
    let _ = sym.register(&equip);
    sym
}

fn equip_table() -> DefTable {
    DefTable {
        name: "game.TbEquip".into(),
        module: "game".into(),
        comment: None,
        mode: liuhuo_core::defs::TableMode::Map,
        index: vec![TableIndex { columns: vec!["id".into()] }],
        value_type: "game.EquipCfg".into(),
        input: vec!["equip.lhd".into()],
        groups: vec![],
    }
}

fn header_ok() -> String {
    let sym = build_symtab();
    format!(
        "## format=lhd\n## version=1\n## table=TbEquip\n## record=game.EquipCfg\n## fields=id|name|quality|atk|tags|attr\n## order=id\n## schema={}\n",
        schema_fingerprint(&equip_table(), &sym)
    )
}

// ============================================================================
// A. 头部指令
// ============================================================================

fn scenario_header() {
    println!("\n=== A. 头部指令 ===");
    let sym = build_symtab();
    let doc = format!(
        "{}\n{{1|\"铁剑\"|Green|10|[\"武器\"]|{{锐利=5}}}}",
        header_ok()
    );
    let r = load_lhd_from_str(&doc, &equip_table(), &sym).unwrap();
    check("A", "标准头部加载成功", r.data.len() == 1 && r.diagnostics.iter().all(|d| !d.is_error()),
        &format!("{:?}", r.diagnostics));

    // 自定义元数据 + 未知指令容忍
    let doc2 = format!(
        "## format=lhd\n## version=1\n## table=TbEquip\n## record=game.EquipCfg\n## fields=id|name|quality|atk|tags|attr\n## order=id\n## @author lyra\n## future=xyz\n\n{{1|\"a\"|Green|1|[]|{{}}}}",
    );
    let r2 = load_lhd_from_str(&doc2, &equip_table(), &sym).unwrap();
    check("A", "自定义 @元数据与未知指令容忍", r2.data.len() == 1 && r2.header.custom.len() == 1,
        &format!("custom={:?}", r2.header.custom));

    // 缺 fields → 报错
    let doc3 = "## format=lhd\n## version=1\n\n{1}\n";
    let r3 = load_lhd_from_str(doc3, &equip_table(), &sym);
    check("A", "缺 ## fields 指令报错", r3.is_err(), &format!("{:?}", r3.err()));
}

// ============================================================================
// B. 行加载（类型指导）
// ============================================================================

fn scenario_rows() {
    println!("\n=== B. 行加载 ===");
    let sym = build_symtab();
    let doc = format!(
        "{}\n// 全类型行\n{{1001|\"魔典\"|Purple|15|[\"武器\",\"法器\"]|{{强化=9,诅咒=2}}}}\n{{1002|\"空容器\"|White|0|[]|{{}}}}",
        header_ok()
    );
    let r = load_lhd_from_str(&doc, &equip_table(), &sym).unwrap();
    check("B", "嵌套容器行加载（list+map）", r.data.len() == 2, &format!("{:?}", r.diagnostics));

    let rec = &r.data.records[0];
    check("B", "id int 解析", matches!(rec.data[0], DType::Int(1001)), &format!("{:?}", rec.data[0]));
    check("B", "枚举裸名 Purple→3", matches!(&rec.data[2], DType::Enum(_, v) if *v == 3),
        &format!("{:?}", rec.data[2]));
    match &rec.data[4] {
        DType::List(items) => check("B", "list<string> 解析 2 项",
            items.len() == 2 && matches!(&items[0], DType::Str(s) if s == "武器"), &format!("{:?}", rec.data[4])),
        other => check("B", "list<string> 解析 2 项", false, &format!("{:?}", other)),
    }
    match &rec.data[5] {
        DType::Map(m) => check("B", "map<string,int> {k=v} 解析",
            m.len() == 2 && matches!(&m[0].1, DType::Int(9)), &format!("{:?}", rec.data[5])),
        other => check("B", "map<string,int> {k=v} 解析", false, &format!("{:?}", other)),
    }
    check("B", "空容器 () 与 {} 解析",
        matches!(&r.data.records[1].data[4], DType::List(v) if v.is_empty())
            && matches!(&r.data.records[1].data[5], DType::Map(m) if m.is_empty()),
        "空容器解析失败");

    // 数字格式：0x / 下划线 / 负数
    let doc2 = format!("{}\n{{0x10|\"甲\"|Green|1_000|[]|{{}}}}", header_ok());
    let r2 = load_lhd_from_str(&doc2, &equip_table(), &sym).unwrap();
    check("B", "0x/下划线整数（0x10=16, 1_000=1000）",
        matches!(&r2.data.records[0].data[0], DType::Int(16)) && matches!(&r2.data.records[0].data[3], DType::Int(1000)),
        &format!("{:?}", r2.data.records.get(0).map(|r| r.data.clone())));

    // 类型错误逐行收集
    let doc3 = format!("{}\n{{1|\"a\"|Green|10|[]|{{}}}}\n{{2|\"b\"|Green|\"不是数字\"|[]|{{}}}}\n{{3|\"c\"|Rainbow|10|[]|{{}}}}", header_ok());
    let r3 = load_lhd_from_str(&doc3, &equip_table(), &sym).unwrap();
    check("B", "逐行错误收集（好行保留+2条错误）",
        r3.data.len() == 1 && r3.diagnostics.iter().filter(|d| d.is_error()).count() >= 2,
        &format!("data={} diags={:?}", r3.data.len(), r3.diagnostics));
    check("B", "错误定位含行号", r3.diagnostics.iter().any(|d| d.message.contains("行10") || d.message.contains("行11")),
        &format!("{:?}", r3.diagnostics));
}

// ============================================================================
// C. 停用行
// ============================================================================

fn scenario_disabled() {
    println!("\n=== C. 停用行 ===");
    let sym = build_symtab();
    let doc = format!(
        "{}\n{{1|\"铁剑\"|Green|10|[]|{{}}}}\n:{{2|\"旧木盾\"|White|3|[]|{{}}}} // 已停用\n:{{3|\"布甲\"|Blue|0|[]|{{}}}} @tag(removed)\n{{4|\"紫金冠\"|Purple|0|[]|{{}}}}",
        header_ok()
    );
    let r = load_lhd_from_str(&doc, &equip_table(), &sym).unwrap();
    check("C", "停用行不进数据（2 启用/2 停用）",
        r.data.len() == 2 && r.disabled.len() == 2, &format!("{} / {}", r.data.len(), r.disabled.len()));
    check("C", "停用行内容仍可解析",
        matches!(&r.disabled[0].data[0], DType::Int(2)) && r.disabled[1].tags.contains_key("removed"),
        &format!("{:?}", r.disabled));

    // 保存：停用行原位保留（主键排序下落在键序位置）
    let saved = save_lhd(&equip_table(), &r.data, &r.disabled, &sym, &[]);
    check("C", "保存保留 : 停用行", saved.lines().filter(|l| l.starts_with(":{")).count() == 2,
        &saved);
    // 往返后仍 2+2
    let r2 = load_lhd_from_str(&saved, &equip_table(), &sym).unwrap();
    check("C", "往返后停用行身份不变", r2.data.len() == 2 && r2.disabled.len() == 2,
        &format!("{} / {}", r2.data.len(), r2.disabled.len()));
}

// ============================================================================
// D. 数据标签
// ============================================================================

fn scenario_tags() {
    println!("\n=== D. 数据标签 ===");
    let sym = build_symtab();
    let doc = format!(
        "{}\n{{1|\"a\"|Green|1|[]|{{}}}} @tag(dev,release)\n{{2|\"b\"|Green|1|[]|{{}}}} @tag(stage=alpha)\n{{3|\"c\"|Green|1|[]|{{}}}}",
        header_ok()
    );
    let r = load_lhd_from_str(&doc, &equip_table(), &sym).unwrap();
    let t0 = &r.data.records[0].tags;
    let t1 = &r.data.records[1].tags;
    check("D", "多标签 @tag(dev,release)", t0.contains_key("dev") && t0.contains_key("release"),
        &format!("{:?}", t0));
    check("D", "键值标签 @tag(stage=alpha)", t1.get("stage").map(|s| s.as_str()) == Some("alpha"),
        &format!("{:?}", t1));
    check("D", "无标签行 tags 为空", r.data.records[2].tags.is_empty(), "");

    // 字符串值内的 @tag( 不是行标签（在引号内）
    let doc2 = format!(
        "{}\n{{1|\"价格@tag(x)见商店\"|Green|1|[]|{{}}}}",
        header_ok()
    );
    let r2 = load_lhd_from_str(&doc2, &equip_table(), &sym).unwrap();
    check("D", "字符串内 @tag 不误判",
        r2.data.len() == 1 && r2.data.records[0].tags.is_empty()
            && matches!(&r2.data.records[0].data[1], DType::Str(s) if s.contains("@tag")),
        &format!("{:?} tags={:?}", r2.data.records.get(0).map(|r| r.data[1].clone()), r2.data.records.first().map(|r| r.tags.clone())));
}

// ============================================================================
// E. 注释
// ============================================================================

fn scenario_comments() {
    println!("\n=== E. 注释 ===");
    let sym = build_symtab();
    let doc = format!(
        "{}\n// 整行注释：装备表 v2\n{{1|\"url含//斜杠\"|Green|1|[]|{{}}}} // 行尾注释\n{{2|\"b\"|Green|1|[]|{{}}}}",
        header_ok()
    );
    let r = load_lhd_from_str(&doc, &equip_table(), &sym).unwrap();
    check("E", "整行注释跳过 + 行尾注释剥离", r.data.len() == 2, &format!("{:?}", r.diagnostics));
    check("E", "字符串内 // 保留",
        matches!(&r.data.records[0].data[1], DType::Str(s) if s == "url含//斜杠"),
        &format!("{:?}", r.data.records[0].data[1]));
}

// ============================================================================
// F. 多态行
// ============================================================================

fn scenario_polymorphic() {
    println!("\n=== F. 多态行 ===");
    let mut sym = build_symtab();
    let weapon: RawDef = serde_json::from_str(
        r#"{"Bean":{"name":"WeaponCfg","module":"game","parent":"game.EquipCfg","fields":[{"name":"range","type":"int"}]}}"#,
    )
    .unwrap();
    let _ = sym.register(&weapon);
    let table = DefTable {
        mode: liuhuo_core::defs::TableMode::Map,
        ..equip_table()
    };
    // 多态表：record=EquipCfg（父），子类 WeaponCfg 多一个 range 字段
    let head = format!(
        "## format=lhd\n## version=1\n## table=TbEquip\n## record=game.EquipCfg\n## fields=id|name|quality|atk|tags|attr\n## order=id\n## schema={}\n",
        schema_fingerprint(&table, &sym)
    );
    let doc = format!(
        "{}\n@type(game.WeaponCfg){{1|\"弓\"|Green|10|[]|{{}}|8}}\n{{2|\"帽\"|White|0|[]|{{}}}}",
        head
    );
    let r = load_lhd_from_str(&doc, &table, &sym).unwrap();
    check("F", "@type 行解析为子类（7 字段）",
        r.data.len() == 2 && r.data.records[0].bean.as_deref() == Some("game.WeaponCfg"),
        &format!("{:?}", r.diagnostics));
    check("F", "无标记行默认 record 类型",
        r.data.records[1].bean.as_deref() == Some("game.EquipCfg"), "");

    // @type 缺闭合括号 → 行级错误
    let doc2 = format!("{}\n@type(game.WeaponCfg{{1|\"x\"|Green|1|[]|{{}}}}", head);
    let r2 = load_lhd_from_str(&doc2, &table, &sym).unwrap();
    check("F", "@type 缺 ) 报行级错误", r2.diagnostics.iter().any(|d| d.is_error() && d.message.contains("@type")),
        &format!("{:?}", r2.diagnostics));
}

// ============================================================================
// G. schema 漂移
// ============================================================================

fn scenario_drift() {
    println!("\n=== G. schema 漂移 ===");
    let sym = build_symtab();
    // 1. fields 错位 → 硬错误 + 不加载数据
    let doc = header_ok().replace("id|name|quality|atk|tags|attr", "id|name|quality|atk|tags|attrs");
    let doc = format!("{}\n{{1|\"a\"|Green|1|[]|{{}}}}", doc);
    let r = load_lhd_from_str(&doc, &equip_table(), &sym).unwrap();
    check("G", "fields 错位 = 硬错误", r.diagnostics.iter().any(|d| d.is_error()), &format!("{:?}", r.diagnostics));
    check("G", "fields 错位不加载数据（防静默错位）", r.data.is_empty(), &format!("{}", r.data.len()));

    // 2. 字段数不同 → 硬错误
    let doc2 = header_ok().replace("id|name|quality|atk|tags|attr", "id|name|quality");
    let doc2 = format!("{}\n{{1|\"a\"|Green}}", doc2);
    let r2 = load_lhd_from_str(&doc2, &equip_table(), &sym).unwrap();
    check("G", "字段数不一致 = 硬错误", r2.diagnostics.iter().any(|d| d.is_error() && d.message.contains("列")), &format!("{:?}", r2.diagnostics));

    // 3. 指纹不匹配 → 警告不阻断
    let doc3 = header_ok().replace(&format!("## schema={}", schema_fingerprint(&equip_table(), &sym)), "## schema=deadbeef");
    let doc3 = format!("{}\n{{1|\"a\"|Green|1|[]|{{}}}}", doc3);
    let r3 = load_lhd_from_str(&doc3, &equip_table(), &sym).unwrap();
    check("G", "指纹不匹配 = 警告且数据正常加载",
        r3.data.len() == 1 && r3.diagnostics.iter().any(|d| !d.is_error() && d.message.contains("指纹")),
        &format!("{:?}", r3.diagnostics));

    // 4. schema 真实变更（加字段）→ fields 核对报错
    let mut sym2 = build_symtab();
    let equip_v2: RawDef = serde_json::from_str(
        r#"{"Bean":{"name":"EquipCfg","module":"game","parent":"game.ItemBase","fields":[{"name":"quality","type":"Quality"},{"name":"atk","type":"int"},{"name":"tags","type":"list<string>"},{"name":"attr","type":"map<string,int>"},{"name":"level","type":"int"}]}}"#,
    )
    .unwrap();
    let _ = sym2.update(&equip_v2);
    let doc4 = format!("{}\n{{1|\"a\"|Green|1|[]|{{}}}}", header_ok());
    let r4 = load_lhd_from_str(&doc4, &equip_table(), &sym2).unwrap();
    check("G", "schema 加字段后旧数据文件报错", r4.diagnostics.iter().any(|d| d.is_error()),
        &format!("{:?}", r4.diagnostics));
}

// ============================================================================
// H. 确定性保存 + 性能
// ============================================================================

// ============================================================================
// I. 严格语法规则（v1 定稿）
// ============================================================================

fn scenario_strict() {
    println!("
=== I. 严格语法规则 ===");
    let sym = build_symtab();
    let head = header_ok();

    // 1. 字典只允许 k=v：k:v 报错
    let doc = format!("{}
{{1|\"a\"|Green|1|[]|{{k:5}}}}", head);
    let r = load_lhd_from_str(&doc, &equip_table(), &sym).unwrap();
    check("I", "字典 k:v 报错（只允许 k=v）",
        r.diagnostics.iter().any(|d| d.is_error() && d.message.contains("k=v")),
        &format!("{:?}", r.diagnostics));
    // k=v 裸键合法
    let doc = format!("{}
{{1|\"a\"|Green|1|[]|{{k=5}}}}", head);
    let r = load_lhd_from_str(&doc, &equip_table(), &sym).unwrap();
    check("I", "字典 k=v 裸键合法", r.data.len() == 1, &format!("{:?}", r.diagnostics));

    // 2. 数字不能带引号
    let doc = format!("{}
{{\"1\"|\"a\"|Green|1|[]|{{}}}}", head);
    let r = load_lhd_from_str(&doc, &equip_table(), &sym).unwrap();
    check("I", "数字带引号报错（含修复提示）",
        r.diagnostics.iter().any(|d| d.is_error() && d.message.contains("数字不能用双引号")),
        &format!("{:?}", r.diagnostics));

    // 3. 枚举不能带引号
    let doc = format!("{}
{{1|\"a\"|\"Green\"|1|[]|{{}}}}", head);
    let r = load_lhd_from_str(&doc, &equip_table(), &sym).unwrap();
    check("I", "枚举带引号报错",
        r.diagnostics.iter().any(|d| d.is_error() && d.message.contains("枚举不能用双引号")),
        &format!("{:?}", r.diagnostics));

    // 4. 字符串必须带引号
    let doc = format!("{}
{{1|裸字符串|Green|1|[]|{{}}}}", head);
    let r = load_lhd_from_str(&doc, &equip_table(), &sym).unwrap();
    check("I", "裸字符串报错",
        r.diagnostics.iter().any(|d| d.is_error() && d.message.contains("双引号")),
        &format!("{:?}", r.diagnostics));

    // 5. 首尾空格自动过滤（字段间多余空白）
    let doc = format!("{}
{{  1  |  \"铁剑\"  |  Green  |  10  |  []  |  {{}}  }}", head);
    let r = load_lhd_from_str(&doc, &equip_table(), &sym).unwrap();
    check("I", "首尾空格自动过滤",
        r.data.len() == 1 && matches!(&r.data.records[0].data[0], DType::Int(1))
            && matches!(&r.data.records[0].data[3], DType::Int(10)),
        &format!("{:?}", r.diagnostics));

    // 6. bool 裸形式（好行）+ 引号形式（坏行）
    let mut sym2 = build_symtab();
    let bean: RawDef = serde_json::from_str(
        r#"{"Bean":{"name":"FlagCfg","module":"game","fields":[{"name":"id","type":"int"},{"name":"on","type":"bool"}]}}"#,
    ).unwrap();
    let _ = sym2.register(&bean);
    let t2 = DefTable {
        name: "game.TbFlag".into(),
        value_type: "game.FlagCfg".into(),
        index: vec![TableIndex { columns: vec!["id".into()] }],
        ..equip_table()
    };
    let head2 = format!(
        "## format=lhd
## version=1
## table=TbFlag
## record=game.FlagCfg
## fields=id|on
## order=id
## schema={}
",
        schema_fingerprint(&t2, &sym2)
    );
    let doc = format!("{}
{{1|true}}
{{2|\"false\"}}", head2);
    let r = load_lhd_from_str(&doc, &t2, &sym2).unwrap();
    check("I", "bool 裸 true 合法 / 引号 false 报错",
        r.data.len() == 1 && matches!(&r.data.records[0].data[1], DType::Bool(true))
            && r.diagnostics.iter().any(|d| d.is_error() && d.message.contains("bool 不能用双引号")),
        &format!("data={} diags={:?}", r.data.len(), r.diagnostics));
}

fn scenario_save_perf(root: &Path) {
    println!("\n=== H. 确定性保存 + 性能 ===");
    let sym = build_symtab();

    // 1. 幂等往返
    let doc = format!(
        "{}\n{{3|\"c\"|Blue|30|[]|{{}}}} @tag(dev)\n{{1|\"a\"|Green|10|[]|{{}}}}\n:{{5|\"e\"|White|50|[]|{{}}}}\n{{2|\"b\"|Green|20|[]|{{}}}}",
        header_ok()
    );
    let r = load_lhd_from_str(&doc, &equip_table(), &sym).unwrap();
    let t1 = save_lhd(&equip_table(), &r.data, &r.disabled, &sym, &[]);
    let r2 = load_lhd_from_str(&t1, &equip_table(), &sym).unwrap();
    let t2 = save_lhd(&equip_table(), &r2.data, &r2.disabled, &sym, &[]);
    check("H", "保存幂等（二次往返字节级一致）", t1 == t2, "t1 != t2");
    let ids: Vec<&str> = t1.lines().filter(|l| l.starts_with("{")).collect();
    check("H", "乱序主键重排为 1,2,3",
        ids.len() == 3 && ids[0].starts_with("{1|") && ids[1].starts_with("{2|") && ids[2].starts_with("{3|"),
        &format!("{:?}", ids));

    // 2. list 表 order=- 保留人工顺序
    let list_table = DefTable {
        mode: liuhuo_core::defs::TableMode::List,
        index: vec![TableIndex { columns: vec!["id".into(), "quality".into()] }],
        ..equip_table()
    };
    let head_l = format!(
        "## format=lhd\n## version=1\n## table=TbEquipList\n## record=game.EquipCfg\n## fields=id|name|quality|atk|tags|attr\n## order=-\n"
    );
    let docl = format!(
        "{}\n{{3|\"c\"|Blue|30|[]|{{}}}}\n{{1|\"a\"|Green|10|[]|{{}}}}\n{{2|\"b\"|Green|20|[]|{{}}}}",
        head_l
    );
    let rl = load_lhd_from_str(&docl, &list_table, &sym).unwrap();
    let tl = save_lhd(&list_table, &rl.data, &rl.disabled, &sym, &[]);
    let idsl: Vec<&str> = tl.lines().filter(|l| l.starts_with("{")).collect();
    check("H", "order=- 保留人工顺序（3,1,2 不重排）",
        idsl.len() == 3 && idsl[0].starts_with("{3|") && idsl[2].starts_with("{2|"),
        &format!("{:?}", idsl));

    // 3. 自定义元数据保存
    let tm = save_lhd(&equip_table(), &r.data, &r.disabled, &sym, &[("author".to_string(), "lyra".to_string())]);
    check("H", "自定义元数据写回头部", tm.contains("## @author lyra"), &tm);

    // 4. 生成 10 万行大表 → 文件 → LhdDataLoader 加载 → 性能
    let n = 100_000;
    let mut big = String::with_capacity(n * 48);
    big.push_str(&header_ok());
    big.push('\n');
    for i in 0..n {
        big.push_str(&format!("{{{0}|\"item_{0}\"|Green|{0}|[\"t{0}\"]|{{k={0}}}}}\n", i));
    }
    let big_path = root.join("TbBig.lhd");
    std::fs::write(&big_path, &big).unwrap();
    let file_mb = big.len() as f64 / 1024.0 / 1024.0;

    let mut registry = liuhuo_core::DataLoaderRegistry::new();
    registry.register(LhdDataLoader);
    let t0 = Instant::now();
    let loaded = load_table_from_path(&big_path, &equip_table(), &sym, &registry).unwrap();
    let load_ms = t0.elapsed().as_millis();
    check("H", &format!("10万行加载（{} MB, {} ms）", file_mb.round(), load_ms),
        loaded.len() == n && load_ms < 10_000,
        &format!("rows={} ms={}", loaded.len(), load_ms));
    // 首尾行抽查
    check("H", "首行 id=0 尾行 id=99999",
        matches!(&loaded.records[0].data[0], DType::Int(0)) && matches!(&loaded.records[n-1].data[0], DType::Int(99999)),
        "");

    // 保存 10 万行性能
    let t0 = Instant::now();
    let saved_big = save_lhd(&equip_table(), &loaded, &[], &sym, &[]);
    let save_ms = t0.elapsed().as_millis();
    check("H", &format!("10万行确定性保存（{} ms）", save_ms), save_ms < 10_000, &format!("ms={}", save_ms));
    let rb = load_lhd_from_str(&saved_big, &equip_table(), &sym).unwrap();
    let sb = save_lhd(&equip_table(), &rb.data, &rb.disabled, &sym, &[]);
    check("H", "10万行往返幂等", sb == saved_big, "大表往返不一致");
    let _ = DefKind::Enum;
    let _ = compile_enum;
}

fn main() {
    let root = temp_root();
    scenario_header();
    scenario_rows();
    scenario_disabled();
    scenario_tags();
    scenario_comments();
    scenario_polymorphic();
    scenario_drift();
    scenario_strict();
    scenario_save_perf(&root);
    let _ = std::fs::remove_dir_all(&root);

    let pass = PASS.load(Ordering::Relaxed);
    let fail = FAIL.load(Ordering::Relaxed);
    let guard = REPORT.lock().unwrap_or_else(|e| e.into_inner());
    let report = guard.as_ref().unwrap();
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
        "suite": ".lhd 内置数据格式端到端测试",
        "total": pass + fail,
        "passed": pass,
        "failed": fail,
        "verdict": if fail == 0 { "PASS" } else { "FAIL" },
        "groups": groups,
    });
    let p = PathBuf::from("test_scripts/report_lhd.json");
    std::fs::write(&p, serde_json::to_string_pretty(&doc).unwrap()).unwrap();
    println!("\n========================================");
    println!("总计：{} 通过, {} 失败 —— {}", pass, fail, doc["verdict"]);
    println!("报告：{}", p.canonicalize().unwrap().display());
    if fail > 0 {
        exit(1);
    }
}
