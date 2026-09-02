//! 诊断模块 —— GUI 导向的错误收集。
//!
//! 设计理念：编译与校验**收集所有错误**，不 fail-fast。
//! 一次编译返回 `Vec<Diagnostic>`，让前端一次性展示所有问题。

use std::fmt;

/// 诊断级别。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DiagLevel {
    Error,
    Warning,
}

impl DiagLevel {
    pub fn as_str(&self) -> &'static str {
        match self {
            DiagLevel::Error => "error",
            DiagLevel::Warning => "warning",
        }
    }
}

/// 一条诊断。`source` 为定义 full_name 或 `表名[行N].字段` 形式的位置。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub level: DiagLevel,
    pub source: Option<String>,
    pub message: String,
}

impl Diagnostic {
    pub fn error(source: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            level: DiagLevel::Error,
            source: Some(source.into()),
            message: message.into(),
        }
    }

    pub fn warning(source: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            level: DiagLevel::Warning,
            source: Some(source.into()),
            message: message.into(),
        }
    }

    /// 无来源的诊断（全局性错误，如 CLI 参数）。
    pub fn global(level: DiagLevel, message: impl Into<String>) -> Self {
        Self {
            level,
            source: None,
            message: message.into(),
        }
    }

    pub fn is_error(&self) -> bool {
        self.level == DiagLevel::Error
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.source {
            Some(src) => write!(f, "[{}] {}: {}", self.level.as_str(), src, self.message),
            None => write!(f, "[{}] {}", self.level.as_str(), self.message),
        }
    }
}

/// 统计一段诊断中的错误数。
pub fn error_count(diags: &[Diagnostic]) -> usize {
    diags.iter().filter(|d| d.is_error()).count()
}
