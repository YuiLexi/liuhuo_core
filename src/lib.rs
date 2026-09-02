//! LiuHuoCore —— 流火配表工具核心库。
//!
//! 模块：
//! - [`diagnostic`]：诊断（GUI 导向的错误收集）
//! - [`types`]：类型系统 + 类型串解析器
//! - [`defs`]：Raw 层（磁盘 JSON）+ Def 层（编译后）+ 单定义编译
//! - [`symbol`]：符号表 + 依赖图 + 增量编译（P1 核心）

pub mod codegen;
pub mod config;
pub mod data;
pub mod defs;
pub mod diagnostic;
pub mod export;
pub mod formula;
pub mod l10n;
pub mod project;
pub mod symbol;
pub mod text_data;
pub mod types;
pub mod validate;
pub mod value;

pub use codegen::{
    CsCodeGenerator, ICodeGenerator, RustCodeGenerator, TsCodeGenerator, camel_case,
    code_generator, map_type, pascal_case, snake_case,
};
pub use config::{
    CodeTarget, DataTarget, ExportConfig, GroupConfig, LiuHuoConfig, TagFilter, TagFilterMode,
};
pub use data::{DataLoaderRegistry, IDataLoader, JsonDataLoader, load_table_from_path};
pub use defs::{
    DefBean, DefEnum, DefEnumItem, DefField, DefKind, DefRecord, DefTable, DefValue, RawBean,
    RawDef, RawEnum, RawEnumItem, RawField, RawRecord, RawTable, TableIndex, TableMode,
    compile_bean, compile_enum, compile_record, compile_table, full_name, parse_index,
    parse_int_literal,
};
pub use diagnostic::{DiagLevel, Diagnostic, error_count};
pub use export::{IExporter, JsonDataExporter, JsonSchemaExporter, record_to_json};
pub use formula::{
    ComputedColumn, EvalEnv, Expr, apply_formula, compute_columns, eval, parse_expr,
};
pub use l10n::{extract_localization, to_l10n_json, validate_path, validate_paths};
pub use project::{
    CONFIG_FILE, PROJECT_FILE, ProjectCompileOutcome, ProjectInfo, TreeNode, compile_project,
    create_data_file, create_definition_file, create_folder, create_project, delete_path,
    read_compile_cache, read_config, read_project_file, rename_path, save_tree_order, scan_tree,
    write_config, write_project_file,
};
pub use symbol::SymbolTable;
pub use text_data::{TextDataLoader, load_from_str, parse_value, serialize_value, table_to_text};
pub use types::{
    EmptyResolver, MapResolver, TypeInfo, TypeKind, TypeRef, TypeResolver, parse_type,
};
pub use validate::{
    IDataValidator, ITableValidator, RangeValidator, SingleRecordValidator, UniqueKeyValidator,
    ValidatorRegistry, validate_table,
};
pub use value::{DType, DataContext, Record, TableData, update_cell};
