//! 项目工程化：创建/打开项目、目录扫描、文件 CRUD、全量编译管线。
//!
//! 项目目录约定（`project.liuhuo` 标识文件）：
//! ```text
//! <name>/
//!   project.liuhuo          项目标识（JSON）
//!   liuhuo.config.yaml      分组/参数/导出配置
//!   schemas/{enums,beans,records,tables}/*.json  一个定义一个文件
//!   datas/**/*.json         数据文件
//!   .cache/                 tree_order.json + compile_result.json
//! ```

use crate::config::LiuHuoConfig;
use crate::data::{DataLoaderRegistry, JsonDataLoader, load_table_from_path};
use crate::defs::{DefKind, DefTable, RawDef, TableMode};
use crate::diagnostic::Diagnostic;
use crate::symbol::SymbolTable;
use crate::text_data::TextDataLoader;
use crate::types::TypeInfo;
use crate::validate::{ValidatorRegistry, key_string, validate_foreign_keys, validate_table};
use crate::value::{DataContext, TableData};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

pub const PROJECT_FILE: &str = "project.liuhuo";
pub const CONFIG_FILE: &str = "liuhuo.config.yaml";

// ============================================================================
// ProjectInfo
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectInfo {
    pub format: String,
    pub version: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
}

impl ProjectInfo {
    pub fn new(name: &str) -> Self {
        Self {
            format: "liuhuo".into(),
            version: "0.1".into(),
            name: name.into(),
            description: None,
            created_at: Some(unix_secs().to_string()),
        }
    }

    pub fn read(dir: &Path) -> Result<Self, Diagnostic> {
        let path = dir.join(PROJECT_FILE);
        let s = std::fs::read_to_string(&path)
            .map_err(|e| Diagnostic::error(PROJECT_FILE, format!("读取失败: {}", e)))?;
        serde_json::from_str(&s)
            .map_err(|e| Diagnostic::error(PROJECT_FILE, format!("解析失败: {}", e)))
    }

    pub fn write(&self, dir: &Path) -> Result<(), Diagnostic> {
        let s = serde_json::to_string_pretty(self)
            .map_err(|e| Diagnostic::error(PROJECT_FILE, e.to_string()))?;
        std::fs::write(dir.join(PROJECT_FILE), s)
            .map_err(|e| Diagnostic::error(PROJECT_FILE, format!("写入失败: {}", e)))
    }
}

fn unix_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

// ============================================================================
// TreeNode
// ============================================================================

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TreeNode {
    pub name: String,
    pub rel_path: String,
    pub is_dir: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub def_kind: Option<DefKind>,
    pub children: Vec<TreeNode>,
}

// ============================================================================
// 创建 / 打开
// ============================================================================

/// 创建项目骨架（目录 + 标识 + 配置 + git init）。
pub fn create_project(parent_dir: &Path, name: &str) -> Result<ProjectInfo, Diagnostic> {
    let dir = parent_dir.join(name);
    for sub in [
        "schemas/enums",
        "schemas/beans",
        "schemas/records",
        "schemas/tables",
        "datas",
        ".cache",
    ] {
        std::fs::create_dir_all(dir.join(sub))
            .map_err(|e| Diagnostic::error("create_project", format!("创建目录失败: {}", e)))?;
    }

    let info = ProjectInfo::new(name);
    info.write(&dir)?;

    let config = LiuHuoConfig {
        name: name.to_string(),
        ..Default::default()
    };
    write_config(&dir, &config)?;

    // git init（失败仅忽略，不阻断创建）
    let _ = std::process::Command::new("git")
        .arg("init")
        .arg("-q")
        .current_dir(&dir)
        .status();

    Ok(info)
}

// ============================================================================
// 目录扫描
// ============================================================================

/// 扫描项目目录树（schemas + datas）。
pub fn scan_tree(dir: &Path) -> Result<Vec<TreeNode>, Diagnostic> {
    let mut roots = Vec::new();
    if dir.join("schemas").is_dir() {
        roots.push(build_schemas_node(dir));
    }
    if dir.join("datas").is_dir() {
        roots.push(build_generic_node(dir, "datas"));
    }
    Ok(roots)
}

/// schemas/ 下四个子目录，文件带 def_kind。
fn build_schemas_node(dir: &Path) -> TreeNode {
    let kinds = [
        ("enums", DefKind::Enum),
        ("beans", DefKind::Bean),
        ("records", DefKind::Record),
        ("tables", DefKind::Table),
    ];
    let mut children = Vec::new();
    for (sub, kind) in kinds {
        let sub_full = dir.join("schemas").join(sub);
        if !sub_full.is_dir() {
            continue;
        }
        let mut files = Vec::new();
        for name in sorted_entries(&sub_full) {
            if name.ends_with(".json") {
                files.push(TreeNode {
                    name: name.clone(),
                    rel_path: format!("schemas/{}/{}", sub, name),
                    is_dir: false,
                    def_kind: Some(kind),
                    children: Vec::new(),
                });
            }
        }
        children.push(TreeNode {
            name: sub.to_string(),
            rel_path: format!("schemas/{}", sub),
            is_dir: true,
            def_kind: None,
            children: files,
        });
    }
    TreeNode {
        name: "schemas".to_string(),
        rel_path: "schemas".to_string(),
        is_dir: true,
        def_kind: None,
        children,
    }
}

/// 递归扫描任意目录（无 def_kind）。
fn build_generic_node(dir: &Path, rel: &str) -> TreeNode {
    let full = dir.join(rel);
    let mut children = Vec::new();
    for name in sorted_entries(&full) {
        let child_rel = format!("{}/{}", rel, name);
        let child_full = full.join(&name);
        if child_full.is_dir() {
            children.push(build_generic_node(dir, &child_rel));
        } else {
            children.push(TreeNode {
                name,
                rel_path: child_rel,
                is_dir: false,
                def_kind: None,
                children: Vec::new(),
            });
        }
    }
    TreeNode {
        name: rel.to_string(),
        rel_path: rel.to_string(),
        is_dir: true,
        def_kind: None,
        children,
    }
}

fn sorted_entries(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().to_string())
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

// ============================================================================
// 文件 CRUD
// ============================================================================

fn safe_join(dir: &Path, rel_path: &str) -> Result<PathBuf, Diagnostic> {
    let rel = Path::new(rel_path);
    if rel.is_absolute() || rel_path.contains("..") {
        return Err(Diagnostic::error(rel_path, "非法路径"));
    }
    Ok(dir.join(rel))
}

pub fn read_project_file(dir: &Path, rel_path: &str) -> Result<String, Diagnostic> {
    let path = safe_join(dir, rel_path)?;
    std::fs::read_to_string(&path)
        .map_err(|e| Diagnostic::error(rel_path, format!("读取失败: {}", e)))
}

pub fn write_project_file(dir: &Path, rel_path: &str, content: &str) -> Result<(), Diagnostic> {
    let path = safe_join(dir, rel_path)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| Diagnostic::error(rel_path, format!("创建目录失败: {}", e)))?;
    }
    std::fs::write(&path, content)
        .map_err(|e| Diagnostic::error(rel_path, format!("写入失败: {}", e)))
}

pub fn create_definition_file(
    dir: &Path,
    kind: DefKind,
    name: &str,
) -> Result<PathBuf, Diagnostic> {
    let sub = match kind {
        DefKind::Enum => "enums",
        DefKind::Bean => "beans",
        DefKind::Record => "records",
        DefKind::Table => "tables",
    };
    let rel = format!("schemas/{}/{}.json", sub, name);
    let path = safe_join(dir, &rel)?;
    if path.exists() {
        return Err(Diagnostic::error(&rel, "文件已存在"));
    }
    std::fs::create_dir_all(path.parent().unwrap())
        .map_err(|e| Diagnostic::error(&rel, format!("创建目录失败: {}", e)))?;
    std::fs::write(&path, default_def_template(kind, name))
        .map_err(|e| Diagnostic::error(&rel, format!("写入失败: {}", e)))?;
    Ok(path)
}

pub fn create_data_file(dir: &Path, parent_rel: &str, name: &str) -> Result<PathBuf, Diagnostic> {
    let rel = if parent_rel.is_empty() {
        format!("datas/{}.json", name)
    } else {
        format!("datas/{}/{}.json", parent_rel, name)
    };
    let path = safe_join(dir, &rel)?;
    if path.exists() {
        return Err(Diagnostic::error(&rel, "文件已存在"));
    }
    std::fs::create_dir_all(path.parent().unwrap())
        .map_err(|e| Diagnostic::error(&rel, format!("创建目录失败: {}", e)))?;
    std::fs::write(&path, "[]\n")
        .map_err(|e| Diagnostic::error(&rel, format!("写入失败: {}", e)))?;
    Ok(path)
}

pub fn create_folder(dir: &Path, parent_rel: &str, name: &str) -> Result<PathBuf, Diagnostic> {
    let rel = if parent_rel.is_empty() {
        format!("datas/{}", name)
    } else {
        format!("datas/{}/{}", parent_rel, name)
    };
    let path = safe_join(dir, &rel)?;
    std::fs::create_dir_all(&path)
        .map_err(|e| Diagnostic::error(&rel, format!("创建目录失败: {}", e)))?;
    Ok(path)
}

pub fn rename_path(dir: &Path, rel_path: &str, new_name: &str) -> Result<(), Diagnostic> {
    let old = safe_join(dir, rel_path)?;
    if !old.exists() {
        return Err(Diagnostic::error(rel_path, "文件不存在"));
    }
    let parent = old.parent().unwrap().to_path_buf();
    let new = parent.join(new_name);

    // 定义文件重命名时同步内容中的 name
    if rel_path.starts_with("schemas/")
        && old.is_file()
        && let Ok(s) = std::fs::read_to_string(&old)
        && let Ok(mut v) = serde_json::from_str::<serde_json::Value>(&s)
    {
        let base = new_name.trim_end_matches(".json");
        if let Some(obj) = v.as_object_mut() {
            obj.insert(
                "name".to_string(),
                serde_json::Value::String(base.to_string()),
            );
        }
        if let Ok(pretty) = serde_json::to_string_pretty(&v) {
            let _ = std::fs::write(&old, pretty);
        }
    }

    std::fs::rename(&old, &new)
        .map_err(|e| Diagnostic::error(rel_path, format!("重命名失败: {}", e)))
}

pub fn delete_path(dir: &Path, rel_path: &str) -> Result<(), Diagnostic> {
    let path = safe_join(dir, rel_path)?;
    if path.is_dir() {
        std::fs::remove_dir_all(&path)
    } else {
        std::fs::remove_file(&path)
    }
    .map_err(|e| Diagnostic::error(rel_path, format!("删除失败: {}", e)))
}

pub fn save_tree_order(dir: &Path, parent_rel: &str, names: &[String]) -> Result<(), Diagnostic> {
    let cache_dir = dir.join(".cache");
    std::fs::create_dir_all(&cache_dir)
        .map_err(|e| Diagnostic::error("save_tree_order", e.to_string()))?;
    let path = cache_dir.join("tree_order.json");
    let mut map: serde_json::Map<String, serde_json::Value> = std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    map.insert(
        parent_rel.to_string(),
        serde_json::Value::Array(
            names
                .iter()
                .map(|n| serde_json::Value::String(n.clone()))
                .collect(),
        ),
    );
    std::fs::write(&path, serde_json::to_string_pretty(&map).unwrap())
        .map_err(|e| Diagnostic::error("save_tree_order", e.to_string()))
}

fn default_def_template(kind: DefKind, name: &str) -> String {
    match kind {
        DefKind::Enum => format!("{{\n  \"name\": \"{}\",\n  \"items\": []\n}}", name),
        DefKind::Bean => format!("{{\n  \"name\": \"{}\",\n  \"fields\": []\n}}", name),
        DefKind::Table => format!(
            "{{\n  \"name\": \"{}\",\n  \"mode\": \"map\",\n  \"value_type\": \"\"\n}}",
            name
        ),
        DefKind::Record => format!("{{\n  \"name\": \"{}\",\n  \"fields\": []\n}}", name),
    }
}

// ============================================================================
// 配置
// ============================================================================

pub fn read_config(dir: &Path) -> Result<LiuHuoConfig, Diagnostic> {
    let path = dir.join(CONFIG_FILE);
    let s = std::fs::read_to_string(&path)
        .map_err(|e| Diagnostic::error(CONFIG_FILE, format!("读取失败: {}", e)))?;
    LiuHuoConfig::parse_str(&s).map_err(|e| Diagnostic::error(CONFIG_FILE, e))
}

pub fn write_config(dir: &Path, config: &LiuHuoConfig) -> Result<(), Diagnostic> {
    let s = config
        .to_string()
        .map_err(|e| Diagnostic::error(CONFIG_FILE, e))?;
    std::fs::write(dir.join(CONFIG_FILE), s)
        .map_err(|e| Diagnostic::error(CONFIG_FILE, format!("写入失败: {}", e)))
}

// ============================================================================
// 编译管线
// ============================================================================

/// 已加载的表：定义 + 数据 + 字段。
type LoadedTable = (DefTable, TableData, Vec<(String, TypeInfo)>);

/// 编译结果。
#[derive(Debug, Clone)]
pub struct ProjectCompileOutcome {
    pub diagnostics: Vec<Diagnostic>,
    pub schema_error_count: usize,
    pub data_error_count: usize,
    pub total_records: usize,
    pub table_count: usize,
}

impl ProjectCompileOutcome {
    pub fn is_ok(&self) -> bool {
        self.diagnostics.iter().all(|d| !d.is_error())
    }
}

/// 全量编译：多文件 schema 加载 → 编译 → 数据加载 → 校验 → 缓存。
pub fn compile_project(dir: &Path, config: &LiuHuoConfig) -> ProjectCompileOutcome {
    let mut diagnostics = Vec::new();

    // 1. 扫描并加载所有 schema 定义
    let (raws, load_diags) = load_all_schema_defs(dir);
    diagnostics.extend(load_diags);

    // 2. 编译
    let mut symtab = SymbolTable::new();
    let schema_diags = symtab.compile_all(&raws);
    let schema_error_count = schema_diags.iter().filter(|d| d.is_error()).count();
    diagnostics.extend(schema_diags);

    // 3. 数据加载 + 校验（schema 无错才进行）
    let mut total_records = 0;
    let mut data_error_count = 0;
    if schema_error_count == 0 {
        let mut loader_registry = DataLoaderRegistry::new();
        loader_registry.register(JsonDataLoader);
        loader_registry.register(TextDataLoader);
        loader_registry.register(crate::lhd::LhdDataLoader);
        let validator_registry = match config.path_root.as_deref() {
            Some(root) => ValidatorRegistry::with_defaults_and_root(Some(Path::new(root))),
            None => ValidatorRegistry::with_defaults(),
        };

        // 第一阶段：加载所有表数据
        let mut loaded: HashMap<String, LoadedTable> = HashMap::new();
        for table_name in symtab.table_names() {
            let Some(table) = symtab.get_table(&table_name).cloned() else {
                continue;
            };
            let input = table
                .input
                .first()
                .cloned()
                .unwrap_or_else(|| format!("{}.json", table.name));
            let data_path = dir.join("datas").join(&input);
            if !data_path.exists() {
                continue;
            }
            match load_table_from_path(&data_path, &table, &symtab, &loader_registry) {
                Ok(data) => {
                    let fields = symtab
                        .bean_hierarchy_fields(&table.value_type)
                        .unwrap_or_default();
                    loaded.insert(table_name.clone(), (table, data, fields));
                }
                Err(e) => {
                    data_error_count += 1;
                    diagnostics.push(Diagnostic::error(&table_name, e));
                }
            }
        }

        // 第二阶段：建主键索引（map 表）
        let mut key_sets: HashMap<String, HashSet<String>> = HashMap::new();
        for (name, (table, data, fields)) in &loaded {
            if table.mode == TableMode::Map
                && !table.index.is_empty()
                && let Some(col) = table.index[0].columns.first()
                && let Some(pos) = fields.iter().position(|(n, _)| n == col)
            {
                let set: HashSet<String> = data
                    .records
                    .iter()
                    .filter_map(|r| r.data.get(pos))
                    .map(key_string)
                    .collect();
                key_sets.insert(name.clone(), set);
            }
        }

        // 第三阶段：字段校验 + 表校验 + 跨表外键校验
        for (name, (table, data, fields)) in &loaded {
            let vdiags = validate_table(table, data, fields, &validator_registry);
            data_error_count += vdiags.iter().filter(|d| d.is_error()).count();
            diagnostics.extend(vdiags);

            let fk_diags = validate_foreign_keys(name, table, data, fields, &key_sets);
            data_error_count += fk_diags.iter().filter(|d| d.is_error()).count();
            diagnostics.extend(fk_diags);

            total_records += data.len();
        }
    }

    let table_count = symtab.table_count();

    // 4. 缓存
    let cache = serde_json::json!({
        "ok": schema_error_count == 0 && data_error_count == 0,
        "schemaErrorCount": schema_error_count,
        "dataErrorCount": data_error_count,
        "totalRecords": total_records,
        "tableCount": table_count,
    });
    let _ = std::fs::write(dir.join(".cache/compile_result.json"), cache.to_string());

    ProjectCompileOutcome {
        diagnostics,
        schema_error_count,
        data_error_count,
        total_records,
        table_count,
    }
}

/// 写入示例项目文件（示例模板：Quality 枚举 + ItemCfg Bean + TbItem 表 + 数据）。
pub fn write_example(dir: &Path) -> Result<(), Diagnostic> {
    write_project_file(
        dir,
        "schemas/enums/Quality.json",
        r#"{"name":"Quality","comment":"品质","items":[{"name":"White","value":"0"},{"name":"Green"},{"name":"Blue"},{"name":"Purple"},{"name":"Gold"}]}"#,
    )?;
    write_project_file(
        dir,
        "schemas/beans/ItemCfg.json",
        r#"{"name":"ItemCfg","module":"game","comment":"道具配置","fields":[{"name":"id","type":"int"},{"name":"name","type":"string"},{"name":"quality","type":"Quality"},{"name":"price","type":"int(range=[0,9999])"}]}"#,
    )?;
    write_project_file(
        dir,
        "schemas/tables/TbItem.json",
        r#"{"name":"TbItem","module":"game","comment":"道具表","mode":"map","index":"id","value_type":"game.ItemCfg","input":["item.json"]}"#,
    )?;
    write_project_file(
        dir,
        "datas/item.json",
        r#"[{"id":1,"name":"药水","quality":"Green","price":100},{"id":2,"name":"铁剑","quality":"White","price":500}]"#,
    )?;
    Ok(())
}

/// 读取上次编译缓存。
pub fn read_compile_cache(dir: &Path) -> Option<serde_json::Value> {
    let s = std::fs::read_to_string(dir.join(".cache/compile_result.json")).ok()?;
    serde_json::from_str(&s).ok()
}

/// 扫描 schemas/{enums,beans,records,tables}/*.json，加载为 RawDef。
fn load_all_schema_defs(dir: &Path) -> (Vec<RawDef>, Vec<Diagnostic>) {
    let mut raws = Vec::new();
    let mut diags = Vec::new();
    let kinds = [
        ("enums", DefKind::Enum),
        ("beans", DefKind::Bean),
        ("records", DefKind::Record),
        ("tables", DefKind::Table),
    ];
    for (sub, kind) in kinds {
        let sub_dir = dir.join("schemas").join(sub);
        if !sub_dir.is_dir() {
            continue;
        }
        for name in sorted_entries(&sub_dir) {
            if !name.ends_with(".json") {
                continue;
            }
            let path = sub_dir.join(&name);
            let rel = format!("schemas/{}/{}", sub, name);
            match std::fs::read_to_string(&path) {
                Ok(s) => match deserialize_raw(&s, kind) {
                    Ok(raw) => raws.push(raw),
                    Err(e) => diags.push(Diagnostic::error(&rel, e)),
                },
                Err(e) => diags.push(Diagnostic::error(&rel, format!("读取失败: {}", e))),
            }
        }
    }
    (raws, diags)
}

fn deserialize_raw(s: &str, kind: DefKind) -> Result<RawDef, String> {
    match kind {
        DefKind::Enum => serde_json::from_str::<crate::RawEnum>(s)
            .map(RawDef::Enum)
            .map_err(|e| e.to_string()),
        DefKind::Bean => serde_json::from_str::<crate::RawBean>(s)
            .map(RawDef::Bean)
            .map_err(|e| e.to_string()),
        DefKind::Record => serde_json::from_str::<crate::RawRecord>(s)
            .map(RawDef::Record)
            .map_err(|e| e.to_string()),
        DefKind::Table => serde_json::from_str::<crate::RawTable>(s)
            .map(RawDef::Table)
            .map_err(|e| e.to_string()),
    }
}
