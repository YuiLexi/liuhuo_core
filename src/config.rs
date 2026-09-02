//! 项目配置（`liuhuo.config.yaml`）：分组 / 参数 / 导出配置。

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// 项目配置。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LiuHuoConfig {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path_root: Option<String>,
    #[serde(default)]
    pub groups: Vec<GroupConfig>,
    #[serde(default)]
    pub args: HashMap<String, String>,
    #[serde(default)]
    pub exports: Vec<ExportConfig>,
}

impl LiuHuoConfig {
    pub fn parse_str(s: &str) -> Result<Self, String> {
        serde_yaml::from_str(s).map_err(|e| format!("解析配置失败: {}", e))
    }

    pub fn to_string(&self) -> Result<String, String> {
        serde_yaml::to_string(self).map_err(|e| format!("序列化配置失败: {}", e))
    }
}

/// 分组配置。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupConfig {
    pub name: String,
    #[serde(default, skip_serializing_if = "is_false")]
    pub is_default: bool,
    #[serde(default)]
    pub alias: Vec<String>,
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// 导出配置。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportConfig {
    pub name: String,
    /// 参与导出的分组（空 = 全部）。
    #[serde(default)]
    pub groups: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag_filter: Option<TagFilter>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<CodeTarget>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<DataTarget>,
}

/// 标签过滤（include/exclude 互斥）。flat struct 避免 internally-tagged enum 的坑。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TagFilter {
    #[serde(default)]
    pub mode: TagFilterMode,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TagFilterMode {
    #[default]
    None,
    Include,
    Exclude,
}

/// 代码导出目标。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodeTarget {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_module: Option<String>,
    #[serde(default, rename = "type", skip_serializing_if = "Option::is_none")]
    pub code_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dir: Option<String>,
}

/// 数据导出目标。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DataTarget {
    #[serde(default, rename = "type", skip_serializing_if = "Option::is_none")]
    pub data_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dir: Option<String>,
}

// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let config = LiuHuoConfig {
            name: "demo".into(),
            description: Some("测试项目".into()),
            path_root: Some("assets".into()),
            groups: vec![GroupConfig {
                name: "c".into(),
                is_default: true,
                alias: vec!["client".into()],
            }],
            args: [("lang".into(), "zh-CN".into())].into_iter().collect(),
            exports: vec![ExportConfig {
                name: "default".into(),
                groups: vec!["c".into()],
                tag_filter: Some(TagFilter {
                    mode: TagFilterMode::Include,
                    tags: vec!["dev".into()],
                }),
                code: Some(CodeTarget {
                    top_module: Some("cfg".into()),
                    code_type: Some("csharp".into()),
                    dir: Some("out/cs".into()),
                }),
                data: Some(DataTarget {
                    data_type: Some("json".into()),
                    dir: Some("out/json".into()),
                }),
            }],
        };
        let yaml = config.to_string().unwrap();
        let back: LiuHuoConfig = LiuHuoConfig::parse_str(&yaml).unwrap();
        assert_eq!(back.name, "demo");
        assert_eq!(back.path_root.as_deref(), Some("assets"));
        assert_eq!(
            back.exports[0].tag_filter.as_ref().unwrap().mode,
            TagFilterMode::Include
        );
        assert_eq!(back.groups[0].alias, vec!["client"]);
    }
}
