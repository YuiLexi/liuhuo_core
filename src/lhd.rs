//! `.lhd`（LiuHuo Data）—— LiuHuo 内置默认数据格式。
//!
//! 设计文档：`docs/数据格式-lhd.md`。要点：
//! - 文件 = `##` 头部指令块 + 数据行块；一行一记录（git 友好）
//! - 每行 = 一个 record 类型的 **bean 字面量** `{v|v|…}`，解析复用 Bean 分支
//! - 行首 `:` = 停用行（机器可判的数据状态，内容仍合法，不参与编译导出）
//! - 行尾 `@tag(…)` 数据标签；`//` 散文注释（整行或行尾）
//! - `@type(子类)` 多态行标记（`{` 之前）
//! - 保存幂等：主键稳定排序、停用行原位保留、确定性序列化
//!
//! # 文件示例
//! ```text
//! ## format=lhd
//! ## version=1
//! ## table=TbEquip
//! ## record=game.EquipCfg
//! ## fields=id|name|quality|atk|price|tags|attr
//! ## order=id
//! ## schema=a1b2c3d4
//!
//! // 品质枚举: White=0 Green=1 Blue=2 Purple=3
//! {1|"铁剑"|Green|10|100|["武器"]|{锐利=5}}
//! :{9|"旧木盾"|White|3|50|["防具"]|{}}   // 已停用
//! ```

use crate::data::IDataLoader;
use crate::defs::{DefTable, TableMode};
use crate::diagnostic::Diagnostic;
use crate::text_data::{parse_value, serialize_value};
use crate::types::{TypeInfo, TypeKind};
use crate::value::{DataContext, DType, Record, TableData};
use std::collections::HashMap;
use std::path::Path;

pub const LHD_FORMAT: &str = "lhd";
pub const LHD_VERSION: u32 = 1;

// ============================================================================
// 头部
// ============================================================================

/// 解析后的 `.lhd` 头部。
#[derive(Debug, Clone, Default)]
pub struct LhdHeader {
    pub format: String,
    pub version: String,
    pub table: String,
    pub record: String,
    pub fields: Vec<String>,
    /// `## order` 的值（`-` = 保留人工顺序）。
    pub order: String,
    pub schema: Option<String>,
    /// 自定义元数据（`## @key value`），透传。
    pub custom: Vec<(String, String)>,
}

impl LhdHeader {
    /// 按当前 schema 生成头部（保存路径）。
    pub fn from_table(table: &DefTable, ctx: &dyn DataContext, custom: &[(String, String)]) -> Self {
        let fields = ctx
            .bean_hierarchy_fields(&table.value_type)
            .map(|v| v.into_iter().map(|(n, _)| n).collect())
            .unwrap_or_default();
        let order = match table.mode {
            TableMode::Map | TableMode::List => table
                .index
                .first()
                .and_then(|i| i.columns.first().cloned())
                .unwrap_or_else(|| "-".to_string()),
            TableMode::One => "-".to_string(),
        };
        LhdHeader {
            format: LHD_FORMAT.to_string(),
            version: LHD_VERSION.to_string(),
            table: table.name.clone(),
            record: table.value_type.clone(),
            fields,
            order,
            schema: Some(schema_fingerprint(table, ctx)),
            custom: custom.to_vec(),
        }
    }

    /// 渲染为头部文本（确定性：字段顺序固定）。
    pub fn to_text(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!("## format={}\n", self.format));
        out.push_str(&format!("## version={}\n", self.version));
        out.push_str(&format!("## table={}\n", self.table));
        out.push_str(&format!("## record={}\n", self.record));
        out.push_str(&format!("## fields={}\n", self.fields.join("|")));
        out.push_str(&format!("## order={}\n", self.order));
        if let Some(s) = &self.schema {
            out.push_str(&format!("## schema={}\n", s));
        }
        for (k, v) in &self.custom {
            out.push_str(&format!("## @{} {}\n", k, v));
        }
        out
    }
}

/// schema 指纹：record 类型名 + 层级字段名 + 字段类型串的 FNV-1a（32 位十六进制）。
/// 指纹不匹配 = schema 漂移 → 警告（不阻断），提示核对字段映射。
pub fn schema_fingerprint(table: &DefTable, ctx: &dyn DataContext) -> String {
    let mut hash: u32 = 0x811c9dc5;
    fn mix(hash: &mut u32, s: &str) {
        for b in s.bytes() {
            *hash ^= b as u32;
            *hash = hash.wrapping_mul(0x01000193);
        }
    }
    mix(&mut hash, &table.value_type);
    if let Some(fields) = ctx.bean_hierarchy_fields(&table.value_type) {
        for (name, ti) in &fields {
            mix(&mut hash, name);
            mix(&mut hash, &type_kind_text(&ti.kind));
        }
    }
    format!("{:08x}", hash)
}

fn type_kind_text(k: &TypeKind) -> String {
    format!("{:?}", k)
}

/// 头部解析结果：头部 + 消耗的行数。
fn parse_header(text: &str) -> Result<(LhdHeader, usize), String> {
    let mut h = LhdHeader::default();
    let mut fields_raw: Option<String> = None;
    let mut line_no = 0usize;
    for (i, raw) in text.lines().enumerate() {
        let line = raw.trim_end();
        line_no = i + 1;
        let t = line.trim();
        if t.is_empty() || t.starts_with("//") {
            continue;
        }
        if let Some(rest) = t.strip_prefix("##") {
            let rest = rest.trim();
            if let Some(kv) = rest.strip_prefix('@') {
                // ## @key value
                if let Some(sp) = kv.find(char::is_whitespace) {
                    h.custom
                        .push((kv[..sp].trim().to_string(), kv[sp..].trim().to_string()));
                } else {
                    h.custom.push((kv.to_string(), String::new()));
                }
                continue;
            }
            let (k, v) = match rest.find('=') {
                Some(p) => (rest[..p].trim(), rest[p + 1..].trim()),
                None => (rest, ""),
            };
            match k {
                "format" => h.format = v.to_string(),
                "version" => h.version = v.to_string(),
                "table" => h.table = v.to_string(),
                "record" => h.record = v.to_string(),
                "fields" => fields_raw = Some(v.to_string()),
                "order" => h.order = v.to_string(),
                "schema" => h.schema = Some(v.to_string()),
                _ => {} // 未知指令容忍（向前兼容）
            }
            continue;
        }
        // 第一个非头部非注释行 = 头部块结束
        return match fields_raw {
            Some(f) => {
                h.fields = if f.is_empty() {
                    Vec::new()
                } else {
                    f.split('|').map(|s| s.trim().to_string()).collect()
                };
                Ok((h, i))
            }
            None => Err(format!("第 {} 行之前缺少 ## fields 指令", i + 1)),
        };
    }
    match fields_raw {
        Some(f) => {
            h.fields = if f.is_empty() {
                Vec::new()
            } else {
                f.split('|').map(|s| s.trim().to_string()).collect()
            };
            Ok((h, line_no))
        }
        None => Err("文件缺少 ## fields 指令".to_string()),
    }
}

// ============================================================================
// 行级解析
// ============================================================================

/// 一行的解析产物。
#[derive(Debug)]
struct ParsedLine {
    disabled: bool,
    type_marker: Option<String>,
    /// 行主体（首个 `{` 到与之配对的 `}`），已去除 @tag 与注释。
    body: String,
    tags: HashMap<String, String>,
}

/// 剥离行级修饰：停用 `:` / `@type(...)` / 行尾 `@tag(...)` / `// 注释`。
/// 返回 None = 该行不产生记录（空行 / 纯注释行）。
fn parse_line(raw: &str) -> Option<Result<ParsedLine, String>> {
    // 1. 剥行尾注释（引号态之外的首个 //）
    let line = strip_line_comment(raw);
    if line.trim().is_empty() {
        return None;
    }
    // 2. 停用标记
    let (disabled, rest) = match line.trim_start().strip_prefix(':') {
        Some(r) => (true, r.trim_start()),
        None => (false, line.trim()),
    };
    if rest.is_empty() {
        return None;
    }
    // 3. 多态标记 @type(Name)
    let (type_marker, rest) = if let Some(after) = rest.strip_prefix("@type(") {
        match after.find(')') {
            Some(p) => (Some(after[..p].trim().to_string()), after[p + 1..].trim()),
            None => return Some(Err("行首 @type( 缺少闭合 )".to_string())),
        }
    } else {
        (None, rest)
    };
    // 4. 行尾 @tag(...)（配对花括号之外、整个行尾处）
    let (body, tags) = extract_line_tags(rest);
    let body = body.trim().to_string();
    if body.is_empty() {
        return None;
    }
    Some(Ok(ParsedLine {
        disabled,
        type_marker,
        body,
        tags,
    }))
}

/// 引号态感知地剥行尾注释：返回注释之前的部分。
/// `//` 前至少一个空白才算注释（URL 这类字符串不受影响，且字符串内 `//` 不剥离）。
fn strip_line_comment(line: &str) -> &str {
    let bytes = line.as_bytes();
    let mut in_str = false;
    let mut i = 0usize;
    while i < bytes.len() {
        let c = bytes[i] as char;
        if in_str {
            if c == '\\' {
                i += 2; // 跳过转义
                continue;
            }
            if c == '"' {
                in_str = false;
            }
        } else if c == '"' {
            in_str = true;
        } else if c == '/' && i > 0 && bytes[i - 1] == b' ' && i + 1 < bytes.len() && bytes[i + 1] == b'/'
        {
            // 前面有空白且是 "//" —— 但要确认该空白不在字符串内（此处已非字符串态，成立）
            return &line[..i - 1];
        }
        i += 1;
    }
    line
}

/// 行尾标签：`{...} @tag(a,b) @tag(k=v)`。
/// 从行尾向前扫描，收集所有紧随 body 之后的 @tag(...)。
fn extract_line_tags(line: &str) -> (&str, HashMap<String, String>) {
    let mut tags = HashMap::new();
    let mut rest = line.trim_end();
    loop {
        // 找最后一个 @tag( ... )
        let Some(at) = rest.rfind("@tag(") else { break };
        let after = &rest[at + 5..];
        let Some(close) = after.rfind(')') else { break };
        let inner = &after[..close];
        // 必须占满行尾（close 是最后一个字符）才是行尾标签
        if !after[close + 1..].trim().is_empty() {
            break;
        }
        for item in inner.split(',') {
            let item = item.trim();
            if item.is_empty() {
                continue;
            }
            match item.find('=') {
                Some(p) => {
                    tags.insert(item[..p].trim().to_string(), item[p + 1..].trim().to_string())
                }
                None => tags.insert(item.to_string(), String::new()),
            };
        }
        rest = rest[..at].trim_end();
    }
    (rest, tags)
}

// ============================================================================
// 加载
// ============================================================================

/// `.lhd` 加载结果：数据 + 逐行诊断（不快速失败）+ 停用行 + 头部。
pub struct LhdLoadResult {
    pub data: TableData,
    pub diagnostics: Vec<Diagnostic>,
    /// 停用行（解析成功的），GUI 灰显用。
    pub disabled: Vec<Record>,
    pub header: LhdHeader,
}

/// 从文本加载 `.lhd`（主入口，逐行收集诊断）。
pub fn load_lhd_from_str(
    text: &str,
    table: &DefTable,
    ctx: &dyn DataContext,
) -> Result<LhdLoadResult, String> {
    let (header, consumed) = parse_header(text)?;

    // 头部 ↔ schema 核对
    let mut diagnostics = Vec::new();
    let schema_fields = ctx
        .bean_hierarchy_fields(&table.value_type)
        .ok_or_else(|| format!("未知 Bean '{}'", table.value_type))?;
    let schema_names: Vec<&str> = schema_fields.iter().map(|(n, _)| n.as_str()).collect();
    if header.fields != schema_names {
        // 位置对齐比较，给出最精确的错误
        for (i, (f, s)) in header.fields.iter().zip(schema_names.iter()).enumerate() {
            if f.as_str() != *s {
                diagnostics.push(Diagnostic::error(
                    &table.name,
                    format!(
                        "头部 fields 第 {} 列 '{}' 与 schema 的 '{}' 不一致（schema 漂移）",
                        i + 1,
                        f,
                        s
                    ),
                ));
            }
        }
        if header.fields.len() != schema_names.len() {
            diagnostics.push(Diagnostic::error(
                &table.name,
                format!(
                    "头部 fields 共 {} 列，schema 层级字段 {} 个：{}",
                    header.fields.len(),
                    schema_names.len(),
                    schema_names.join("|")
                ),
            ));
        }
        // fields 错位是硬错误：继续加载只会静默错数据
        return Ok(LhdLoadResult {
            data: TableData::new(),
            diagnostics,
            disabled: Vec::new(),
            header,
        });
    }
    if let Some(fp) = &header.schema {
        let expect = schema_fingerprint(table, ctx);
        if *fp != expect {
            diagnostics.push(Diagnostic::warning(
                &table.name,
                format!(
                    "schema 指纹不匹配（文件 {} / 当前 {}）：类型定义已变更，请核对字段映射",
                    fp, expect
                ),
            ));
        }
    }

    let mut data = TableData::new();
    let mut disabled = Vec::new();
    let ti_record = TypeInfo {
        kind: TypeKind::Bean(table.value_type.clone()),
        nullable: false,
        tags: Default::default(),
    };

    for (i, raw) in text.lines().enumerate().skip(consumed) {
        let line_no = i + 1;
        let Some(parsed) = parse_line(raw) else { continue };
        let pl = match parsed {
            Ok(p) => p,
            Err(e) => {
                diagnostics.push(Diagnostic::error(
                    &table.name,
                    format!("{}[行{}] {}", header.table, line_no, e),
                ));
                continue;
            }
        };
        // 行主体按 Bean 解析（类型指导）
        let ti = match &pl.type_marker {
            Some(sub) => TypeInfo {
                kind: TypeKind::Bean(sub.clone()),
                nullable: false,
                tags: Default::default(),
            },
            None => ti_record.clone(),
        };
        match parse_value(&pl.body, &ti, ctx) {
            Ok(DType::Bean(actual, vals)) => {
                let mut rec = Record::with_capacity(vals.len());
                rec.bean = Some(actual);
                rec.data = vals;
                rec.tags = pl.tags;
                if pl.disabled {
                    disabled.push(rec);
                } else {
                    data.push(rec);
                }
            }
            Ok(_) => diagnostics.push(Diagnostic::error(
                &table.name,
                format!("{}[行{}] 行主体应解析为记录值", header.table, line_no),
            )),
            Err(e) => diagnostics.push(Diagnostic::error(
                &table.name,
                format!("{}[行{}] {}", header.table, line_no, e),
            )),
        }
    }

    Ok(LhdLoadResult {
        data,
        diagnostics,
        disabled,
        header,
    })
}

// ============================================================================
// 保存（确定性 / 幂等）
// ============================================================================

/// 渲染一条记录为一行（不含换行）。
pub fn record_to_line(rec: &Record, ctx: &dyn DataContext) -> String {
    let mut out = String::new();
    if let Some(b) = &rec.bean {
        out.push_str(&serialize_value(&DType::Bean(b.clone(), rec.data.clone())));
    } else {
        out.push_str(
            &rec
                .data
                .iter()
                .map(serialize_value)
                .collect::<Vec<_>>()
                .join("|"),
        );
    }
    let _ = ctx;
    if !rec.tags.is_empty() {
        let tags: Vec<String> = rec
            .tags
            .iter()
            .map(|(k, v)| {
                if v.is_empty() {
                    k.clone()
                } else {
                    format!("{}={}", k, v)
                }
            })
            .collect();
        out.push_str(&format!(" @tag({})", tags.join(",")));
    }
    out
}

/// 确定性保存：主键稳定排序（map 表）、停用行原位、头部按当前 schema 刷新。
///
/// `disabled_lines`：停用行（在启用行按主键排序后**按原文件位置**回插）。
/// 简化实现：停用行按其主键值参与排序但保持 `:` 前缀——主键排序天然让它们落在"本该在"的位置。
pub fn save_lhd(
    table: &DefTable,
    data: &TableData,
    disabled: &[Record],
    ctx: &dyn DataContext,
    custom_meta: &[(String, String)],
) -> String {
    let header = LhdHeader::from_table(table, ctx, custom_meta);
    let mut out = header.to_text();
    out.push('\n');

    // 排序键：order 字段在层级字段中的位置
    let fields = ctx
        .bean_hierarchy_fields(&table.value_type)
        .map(|v| v.into_iter().map(|(n, _)| n).collect::<Vec<String>>())
        .unwrap_or_default();
    let order_pos = fields.iter().position(|f| *f == header.order);

    // 启用行 + 停用行统一收集 (排序键, 是否停用, 行文本)
    let mut lines: Vec<(Option<String>, bool, String)> = Vec::new();
    for rec in &data.records {
        let key = order_pos.and_then(|p| rec.data.get(p)).map(key_string);
        lines.push((key, false, record_to_line(rec, ctx)));
    }
    for rec in disabled {
        let key = order_pos.and_then(|p| rec.data.get(p)).map(key_string);
        lines.push((key, true, format!(":{}", record_to_line(rec, ctx))));
    }

    if header.order != "-" {
        // 主键稳定排序（数值键数值序，其它字典序）；无键行保持相对顺序（stable sort）
        lines.sort_by(|a, b| match (&a.0, &b.0) {
            (Some(x), Some(y)) => {
                let nx = x.parse::<i64>().ok();
                let ny = y.parse::<i64>().ok();
                match (nx, ny) {
                    (Some(x), Some(y)) => x.cmp(&y),
                    _ => x.cmp(y),
                }
            }
            (None, None) => std::cmp::Ordering::Equal,
            (None, Some(_)) => std::cmp::Ordering::Less,
            (Some(_), None) => std::cmp::Ordering::Greater,
        });
    }

    for (_, _, text) in &lines {
        out.push_str(text);
        out.push('\n');
    }
    out
}

/// 值 → 排序键字符串（与唯一性校验的 key_string 语义一致）。
fn key_string(v: &DType) -> String {
    match v {
        DType::Int(i) => i.to_string(),
        DType::UInt(u) => u.to_string(),
        DType::Float(f) => format!("{}", *f as i64),
        DType::Str(s) | DType::Text(s) => s.clone(),
        DType::Enum(_, val) => val.to_string(),
        DType::Bool(b) => (*b as i64).to_string(),
        DType::DateTime(d) => d.to_string(),
        _ => String::new(),
    }
}

// ============================================================================
// Loader
// ============================================================================

/// `.lhd` 数据加载器（LiuHuo 内置默认格式）。
#[derive(Debug, Default)]
pub struct LhdDataLoader;

impl IDataLoader for LhdDataLoader {
    fn name(&self) -> &str {
        LHD_FORMAT
    }

    fn extensions(&self) -> &[&str] {
        &["lhd"]
    }

    fn load_table(
        &self,
        path: &Path,
        table: &DefTable,
        ctx: &dyn DataContext,
    ) -> Result<TableData, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("无法读取数据文件 '{}': {}", path.display(), e))?;
        let result = load_lhd_from_str(&text, table, ctx)?;
        // IDataLoader 的 Result 约定：有 Error 级诊断时以 Err 返回首条（GUI 走 load_lhd_from_str 拿全部）
        if let Some(first_err) = result.diagnostics.iter().find(|d| d.is_error()) {
            return Err(first_err.message.clone());
        }
        Ok(result.data)
    }
}

// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// 测试用 DataContext：内存字段表。
    struct Ctx {
        beans: Vec<(String, Vec<(String, TypeInfo)>)>,
        enums: Vec<String>,
    }
    impl Ctx {
        fn add(&mut self, name: &str, fields: Vec<(&str, TypeKind)>) {
            self.beans.push((
                name.to_string(),
                fields
                    .into_iter()
                    .map(|(n, k)| (n.to_string(), TypeInfo::new(k)))
                    .collect(),
            ));
        }
    }
    impl DataContext for Ctx {
        fn enum_value(&self, _: &str, v: &str) -> Option<i64> {
            match v {
                "White" => Some(0),
                "Green" => Some(1),
                "Blue" => Some(2),
                "Purple" => Some(3),
                _ => None,
            }
        }
        fn bean_fields(&self, name: &str) -> Option<Vec<String>> {
            self.beans
                .iter()
                .find(|(n, _)| n == name)
                .map(|(_, fs)| fs.iter().map(|(n, _)| n.clone()).collect())
        }
        fn bean_hierarchy_fields(
            &self,
            name: &str,
        ) -> Option<Vec<(String, TypeInfo)>> {
            self.beans
                .iter()
                .find(|(n, _)| n == name)
                .map(|(_, fs)| fs.clone())
        }
    }

    fn test_table() -> DefTable {
        DefTable {
            name: "game.TbEquip".to_string(),
            module: "game".to_string(),
            comment: None,
            mode: TableMode::Map,
            index: vec![crate::defs::TableIndex {
                columns: vec!["id".to_string()],
            }],
            value_type: "game.EquipCfg".to_string(),
            input: vec!["equip.lhd".to_string()],
            groups: vec![],
        }
    }

    fn ctx() -> Ctx {
        let mut c = Ctx {
            beans: Vec::new(),
            enums: vec!["Quality".to_string()],
        };
        c.add(
            "game.EquipCfg",
            vec![
                ("id", TypeKind::I32),
                ("name", TypeKind::Str),
                ("quality", TypeKind::Enum("Quality".to_string())),
            ],
        );
        c
    }

    const DOC: &str = "## format=lhd
## version=1
## table=TbEquip
## record=game.EquipCfg
## fields=id|name|quality
## order=id
## schema=xxxxxxxx

// 品质: White Green Blue
{1|\"铁剑\"|Green}
{2|\"寒冰弓\"|Blue} @tag(dev)
:{3|\"旧木盾\"|White} // 已停用
{4|\"紫金冠\"|Purple} @tag(stage=alpha)
";

    #[test]
    fn header_parse() {
        let (h, consumed) = parse_header(DOC).unwrap();
        assert_eq!(h.format, "lhd");
        assert_eq!(h.table, "TbEquip");
        assert_eq!(h.record, "game.EquipCfg");
        assert_eq!(h.fields, vec!["id", "name", "quality"]);
        assert_eq!(h.order, "id");
        assert_eq!(consumed, 9); // 头部块延伸至首个数据行之前
    }

    #[test]
    fn load_rows_tags_disabled() {
        let r = load_lhd_from_str(DOC, &test_table(), &ctx()).unwrap();
        assert_eq!(r.data.len(), 3, "启用行 3 条");
        assert_eq!(r.disabled.len(), 1, "停用行 1 条");
        assert!(r.data.records[1].tags.contains_key("dev"));
        assert_eq!(
            r.data.records[2].tags.get("stage").map(|s| s.as_str()),
            Some("alpha")
        );
        match &r.data.records[0].data[0] {
            DType::Int(1) => {}
            other => panic!("id 应为 Int(1): {:?}", other),
        }
        assert!(r
            .diagnostics
            .iter()
            .any(|d| !d.is_error() && d.message.contains("指纹不匹配")));
    }

    #[test]
    fn fields_mismatch_is_error() {
        let doc = DOC.replace("id|name|quality", "id|name|level");
        let r = load_lhd_from_str(&doc, &test_table(), &ctx()).unwrap();
        assert!(r
            .diagnostics
            .iter()
            .any(|d| d.is_error() && d.message.contains("不一致")));
        assert_eq!(r.data.len(), 0, "fields 硬错误时不加载数据");
    }

    #[test]
    fn roundtrip_idempotent() {
        let r = load_lhd_from_str(DOC, &test_table(), &ctx()).unwrap();
        let t1 = save_lhd(&test_table(), &r.data, &r.disabled, &ctx(), &[]);
        let r2 = load_lhd_from_str(&t1, &test_table(), &ctx()).unwrap();
        let t2 = save_lhd(&test_table(), &r2.data, &r2.disabled, &ctx(), &[]);
        assert_eq!(t1, t2, "保存幂等：二次往返字节级一致");
        assert!(t1.contains(":{3|"));
        // 乱序输入按主键排
        let shuffled = "## format=lhd
## version=1
## table=TbEquip
## record=game.EquipCfg
## fields=id|name|quality
## order=id

{3|\"c\"|Green}
{1|\"a\"|Green}
{2|\"b\"|Green}
";
        let r3 = load_lhd_from_str(shuffled, &test_table(), &ctx()).unwrap();
        let t3 = save_lhd(&test_table(), &r3.data, &r3.disabled, &ctx(), &[]);
        let ids: Vec<&str> = t3.lines().filter(|l| l.starts_with('{')).collect();
        assert!(
            ids[0].starts_with("{1|") && ids[2].starts_with("{3|"),
            "主键排序: {:?}",
            ids
        );
    }

    #[test]
    fn comment_and_tag_extraction() {
        let line = r#"{1|"http://x|y"|"a // b"} @tag(dev) // 尾注"#;
        let Some(Ok(pl)) = parse_line(line) else {
            panic!("应解析成功")
        };
        assert!(pl.tags.contains_key("dev"));
        assert!(
            pl.body.contains("http://x|y"),
            "字符串内容保留: {}",
            pl.body
        );
        assert!(!pl.body.contains("尾注"));
    }

    #[test]
    fn polymorphic_line() {
        let mut c = Ctx { beans: Vec::new(), enums: Vec::new() };
        c.add("game.Item", vec![("id", TypeKind::I32)]);
        c.add("game.Weapon", vec![("id", TypeKind::I32)]);
        let t = DefTable {
            value_type: "game.Item".to_string(),
            ..test_table()
        };
        let doc = "## format=lhd
## version=1
## table=Tb
## record=game.Item
## fields=id
## order=-

@type(game.Weapon){100}
{1}
";
        let r = load_lhd_from_str(doc, &t, &c).unwrap();
        assert_eq!(r.data.len(), 2);
        assert_eq!(r.data.records[0].bean.as_deref(), Some("game.Weapon"));
        assert_eq!(r.data.records[1].bean.as_deref(), Some("game.Item"));
    }
}
