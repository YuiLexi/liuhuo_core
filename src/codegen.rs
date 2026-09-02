//! 代码生成：Namer 命名器 + C# / TypeScript / Rust 代码生成器。
//!
//! 设计哲学：根据目标语言特色自动生成命名，特殊命名特殊处理（id/ui/uid/url 等首字母缩写词）。

use crate::defs::{DefValue, TableMode};
use crate::types::{TypeInfo, TypeKind};

// ============================================================================
// Namer —— 命名器
// ============================================================================

/// 首字母缩写词（全大写 / 特殊大小写）的映射。
fn special_word(w: &str) -> Option<&str> {
    match w {
        "id" => Some("Id"),
        "ui" => Some("UI"),
        "uid" => Some("Uid"),
        "url" => Some("Url"),
        "http" => Some("Http"),
        "https" => Some("Https"),
        "api" => Some("Api"),
        "ip" => Some("Ip"),
        "tcp" => Some("Tcp"),
        "udp" => Some("Udp"),
        "gpu" => Some("Gpu"),
        "cpu" => Some("Cpu"),
        "dto" => Some("Dto"),
        "id2" => Some("Id2"),
        _ => None,
    }
}

/// 按 `_`、`-`、大写边界分词。
fn split_words(name: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut cur = String::new();
    for c in name.chars() {
        if c == '_' || c == '-' || c == ' ' {
            if !cur.is_empty() {
                words.push(cur);
                cur = String::new();
            }
        } else if c.is_ascii_uppercase() && !cur.is_empty() {
            words.push(cur);
            cur = c.to_string();
        } else {
            cur.push(c);
        }
    }
    if !cur.is_empty() {
        words.push(cur);
    }
    words
}

fn capitalize(w: &str) -> String {
    if let Some(special) = special_word(&w.to_lowercase()) {
        return special.to_string();
    }
    let mut chars = w.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + &chars.as_str().to_lowercase(),
        None => String::new(),
    }
}

/// PascalCase（类型名 / C# 属性名）。
pub fn pascal_case(name: &str) -> String {
    split_words(name).iter().map(|w| capitalize(w)).collect()
}

/// camelCase（TS 字段名）。
pub fn camel_case(name: &str) -> String {
    let p = pascal_case(name);
    let mut chars = p.chars();
    match chars.next() {
        Some(first) => first.to_lowercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// snake_case（Rust 字段名）。
pub fn snake_case(name: &str) -> String {
    let words = split_words(name);
    words
        .iter()
        .map(|w| w.to_lowercase())
        .collect::<Vec<_>>()
        .join("_")
}

// ============================================================================
// 类型映射
// ============================================================================

/// TypeInfo → 目标语言类型字符串。
pub fn map_type(ti: &TypeInfo, lang: &str) -> String {
    match &ti.kind {
        TypeKind::Bool => match lang {
            "cs" => "bool",
            "ts" => "boolean",
            _ => "bool",
        }
        .into(),
        TypeKind::I8 => match lang {
            "cs" => "sbyte",
            "ts" => "number",
            _ => "i8",
        }
        .into(),
        TypeKind::I16 => match lang {
            "cs" => "short",
            "ts" => "number",
            _ => "i16",
        }
        .into(),
        TypeKind::I32 => match lang {
            "cs" => "int",
            "ts" => "number",
            _ => "i32",
        }
        .into(),
        TypeKind::I64 => match lang {
            "cs" => "long",
            "ts" => "number",
            _ => "i64",
        }
        .into(),
        TypeKind::U8 => match lang {
            "cs" => "byte",
            "ts" => "number",
            _ => "u8",
        }
        .into(),
        TypeKind::U16 => match lang {
            "cs" => "ushort",
            "ts" => "number",
            _ => "u16",
        }
        .into(),
        TypeKind::U32 => match lang {
            "cs" => "uint",
            "ts" => "number",
            _ => "u32",
        }
        .into(),
        TypeKind::U64 => match lang {
            "cs" => "ulong",
            "ts" => "number",
            _ => "u64",
        }
        .into(),
        TypeKind::F32 => match lang {
            "cs" => "float",
            "ts" => "number",
            _ => "f32",
        }
        .into(),
        TypeKind::F64 => match lang {
            "cs" => "double",
            "ts" => "number",
            _ => "f64",
        }
        .into(),
        TypeKind::Str | TypeKind::Text => match lang {
            "ts" => "string",
            _ => "string",
        }
        .into(),
        TypeKind::DateTime => match lang {
            "cs" => "long",
            "ts" => "number",
            _ => "i64",
        }
        .into(),
        TypeKind::Enum(name) | TypeKind::Bean(name) => pascal_case(name),
        TypeKind::Ref(_) => match lang {
            "cs" => "int",
            "ts" => "number",
            _ => "i32",
        }
        .into(),
        TypeKind::Unresolved(name) => format!("/* unresolved: {} */ object", name),
        TypeKind::Array(elem) | TypeKind::List(elem) => match lang {
            "cs" => format!("List<{}>", map_type(elem, lang)),
            "rust" => format!("Vec<{}>", map_type(elem, lang)),
            _ => format!("{}[]", map_type(elem, lang)),
        },
        TypeKind::Set(elem) => match lang {
            "cs" => format!("HashSet<{}>", map_type(elem, lang)),
            "rust" => format!("Vec<{}>", map_type(elem, lang)),
            _ => format!("Set<{}>", map_type(elem, lang)),
        },
        TypeKind::Map(k, v) => match lang {
            "cs" => format!("Dictionary<{},{}>", map_type(k, lang), map_type(v, lang)),
            "rust" => format!("HashMap<{},{}>", map_type(k, lang), map_type(v, lang)),
            _ => format!("Record<{}, {}>", map_type(k, lang), map_type(v, lang)),
        },
    }
}

// ============================================================================
// 代码生成器
// ============================================================================

/// 代码生成器接口。
pub trait ICodeGenerator {
    fn name(&self) -> &str;
    /// 生成代码：返回 (文件名, 内容)。
    fn generate(&self, defs: &[&DefValue]) -> Vec<(String, String)>;
}

/// 枚举代码。
fn gen_enum(enum_name: &str, items: &[(String, i64)], lang: &str) -> String {
    let enum_pascal = pascal_case(enum_name);
    let mut out = String::new();
    match lang {
        "cs" => {
            out.push_str(&format!("public enum {}\n{{\n", enum_pascal));
            for (name, value) in items {
                out.push_str(&format!("    {} = {},\n", pascal_case(name), value));
            }
            out.push_str("}\n");
        }
        "ts" => {
            out.push_str(&format!("export enum {} {{\n", enum_pascal));
            for (name, value) in items {
                out.push_str(&format!("  {} = {},\n", camel_case(name), value));
            }
            out.push_str("}\n");
        }
        _ => {
            out.push_str(&format!("pub enum {} {{\n", enum_pascal));
            for (name, value) in items {
                out.push_str(&format!("    {} = {},\n", pascal_case(name), value));
            }
            out.push_str("}\n");
        }
    }
    out
}

/// Bean / Record 代码。
fn gen_bean(
    name: &str,
    parent: &Option<String>,
    fields: &[&crate::defs::DefField],
    lang: &str,
) -> String {
    let bean_pascal = pascal_case(name);
    let mut out = String::new();
    match lang {
        "cs" => {
            let extends = parent
                .as_ref()
                .map(|p| format!(" : {}", pascal_case(p)))
                .unwrap_or_default();
            out.push_str(&format!("public class {}{}\n{{\n", bean_pascal, extends));
            for f in fields {
                out.push_str(&format!(
                    "    public {} {} {{ get; set; }}\n",
                    map_type(&f.type_info, lang),
                    pascal_case(&f.name)
                ));
            }
            out.push_str("}\n");
        }
        "ts" => {
            let extends = parent
                .as_ref()
                .map(|p| format!(" extends {}", pascal_case(p)))
                .unwrap_or_default();
            out.push_str(&format!("export interface {}{} {{\n", bean_pascal, extends));
            for f in fields {
                out.push_str(&format!(
                    "  {}: {};\n",
                    camel_case(&f.name),
                    map_type(&f.type_info, lang)
                ));
            }
            out.push_str("}\n");
        }
        _ => {
            out.push_str(&format!(
                "#[derive(Debug, Clone)]\npub struct {} {{\n",
                bean_pascal
            ));
            for f in fields {
                out.push_str(&format!(
                    "    pub {}: {},\n",
                    snake_case(&f.name),
                    map_type(&f.type_info, lang)
                ));
            }
            out.push_str("}\n");
        }
    }
    out
}

/// C# 代码生成器。
#[derive(Debug, Default)]
pub struct CsCodeGenerator;

impl ICodeGenerator for CsCodeGenerator {
    fn name(&self) -> &str {
        "cs"
    }
    fn generate(&self, defs: &[&DefValue]) -> Vec<(String, String)> {
        let mut out = String::new();
        out.push_str("// 由 LiuHuo 自动生成\n\n");
        for def in defs {
            match def {
                DefValue::Enum(e) => {
                    let items: Vec<(String, i64)> =
                        e.items.iter().map(|i| (i.name.clone(), i.value)).collect();
                    out.push_str(&gen_enum(&e.name, &items, "cs"));
                    out.push('\n');
                }
                DefValue::Bean(b) => {
                    out.push_str(&gen_bean(
                        &b.name,
                        &b.parent,
                        &b.hierarchy_fields.iter().collect::<Vec<_>>(),
                        "cs",
                    ));
                    out.push('\n');
                }
                DefValue::Record(r) => {
                    out.push_str(&gen_bean(
                        &r.name,
                        &None,
                        &r.fields.iter().collect::<Vec<_>>(),
                        "cs",
                    ));
                    out.push('\n');
                }
                DefValue::Table(_) => {}
            }
        }
        vec![("Config.cs".to_string(), out)]
    }
}

/// TypeScript 代码生成器。
#[derive(Debug, Default)]
pub struct TsCodeGenerator;

impl ICodeGenerator for TsCodeGenerator {
    fn name(&self) -> &str {
        "ts"
    }
    fn generate(&self, defs: &[&DefValue]) -> Vec<(String, String)> {
        let mut out = String::new();
        out.push_str("// 由 LiuHuo 自动生成\n\n");
        for def in defs {
            match def {
                DefValue::Enum(e) => {
                    let items: Vec<(String, i64)> =
                        e.items.iter().map(|i| (i.name.clone(), i.value)).collect();
                    out.push_str(&gen_enum(&e.name, &items, "ts"));
                    out.push('\n');
                }
                DefValue::Bean(b) => {
                    out.push_str(&gen_bean(
                        &b.name,
                        &b.parent,
                        &b.hierarchy_fields.iter().collect::<Vec<_>>(),
                        "ts",
                    ));
                    out.push('\n');
                }
                DefValue::Record(r) => {
                    out.push_str(&gen_bean(
                        &r.name,
                        &None,
                        &r.fields.iter().collect::<Vec<_>>(),
                        "ts",
                    ));
                    out.push('\n');
                }
                DefValue::Table(_) => {}
            }
        }
        vec![("config.ts".to_string(), out)]
    }
}

/// Rust 代码生成器。
#[derive(Debug, Default)]
pub struct RustCodeGenerator;

impl ICodeGenerator for RustCodeGenerator {
    fn name(&self) -> &str {
        "rust"
    }
    fn generate(&self, defs: &[&DefValue]) -> Vec<(String, String)> {
        let mut out = String::new();
        out.push_str("// 由 LiuHuo 自动生成\n\n");
        for def in defs {
            match def {
                DefValue::Enum(e) => {
                    let items: Vec<(String, i64)> =
                        e.items.iter().map(|i| (i.name.clone(), i.value)).collect();
                    out.push_str(&gen_enum(&e.name, &items, "rust"));
                    out.push('\n');
                }
                DefValue::Bean(b) => {
                    out.push_str(&gen_bean(
                        &b.name,
                        &b.parent,
                        &b.hierarchy_fields.iter().collect::<Vec<_>>(),
                        "rust",
                    ));
                    out.push('\n');
                }
                DefValue::Record(r) => {
                    out.push_str(&gen_bean(
                        &r.name,
                        &None,
                        &r.fields.iter().collect::<Vec<_>>(),
                        "rust",
                    ));
                    out.push('\n');
                }
                DefValue::Table(_) => {}
            }
        }
        vec![("config.rs".to_string(), out)]
    }
}

/// 按语言名取代码生成器。
pub fn code_generator(lang: &str) -> Option<Box<dyn ICodeGenerator>> {
    match lang {
        "cs" | "csharp" => Some(Box::new(CsCodeGenerator)),
        "ts" | "typescript" => Some(Box::new(TsCodeGenerator)),
        "rust" => Some(Box::new(RustCodeGenerator)),
        _ => None,
    }
}

// 表模式（供未来 table 访问器生成）
#[allow(unused)]
fn _table_mode(m: TableMode) -> &'static str {
    match m {
        TableMode::One => "one",
        TableMode::Map => "map",
        TableMode::List => "list",
    }
}

// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn namer_special_words() {
        assert_eq!(pascal_case("id"), "Id");
        assert_eq!(pascal_case("ui"), "UI");
        assert_eq!(pascal_case("uid"), "Uid");
        assert_eq!(pascal_case("url"), "Url");
        assert_eq!(pascal_case("user_name"), "UserName");
        assert_eq!(camel_case("user_name"), "userName");
        assert_eq!(camel_case("id"), "id");
        assert_eq!(snake_case("userName"), "user_name");
        assert_eq!(pascal_case("item_cfg"), "ItemCfg");
    }

    #[test]
    fn map_type_per_lang() {
        let i32_ti = TypeInfo::new(TypeKind::I32);
        assert_eq!(map_type(&i32_ti, "cs"), "int");
        assert_eq!(map_type(&i32_ti, "ts"), "number");
        assert_eq!(map_type(&i32_ti, "rust"), "i32");

        let list_ti = TypeInfo::new(TypeKind::List(Box::new(TypeInfo::new(TypeKind::I32))));
        assert_eq!(map_type(&list_ti, "cs"), "List<int>");
        assert_eq!(map_type(&list_ti, "ts"), "number[]");
        assert_eq!(map_type(&list_ti, "rust"), "Vec<i32>");

        let enum_ti = TypeInfo::new(TypeKind::Enum("quality".into()));
        assert_eq!(map_type(&enum_ti, "cs"), "Quality");
    }

    #[test]
    fn gen_enum_outputs() {
        let items = vec![("white".to_string(), 0), ("green".to_string(), 1)];
        let cs = gen_enum("Quality", &items, "cs");
        assert!(cs.contains("public enum Quality"));
        assert!(cs.contains("White = 0"));

        let ts = gen_enum("Quality", &items, "ts");
        assert!(ts.contains("export enum Quality"));
        assert!(ts.contains("white = 0"));
    }
}
