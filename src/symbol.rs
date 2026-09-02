//! 符号表 + 依赖图 + 增量编译 —— P1 核心。
//!
//! # 核心思想
//!
//! 编译产物（`defs`）只是**缓存**，符号表是**可增删改的索引**。增删改通过依赖图
//! 做**反向失效传播**（BFS），只重编译受影响的最小集合 —— 把 Luban 的"每次全量
//! 三阶段编译"降级为"增量重校验"。
//!
//! # 五个核心操作
//!
//! - `register`：创建即编译（含"引用不存在类型"的即时诊断）
//! - `update`：编辑即重编译 + 反向重检所有依赖者
//! - `remove`：删除即失效（依赖者立即产生"未解析类型"诊断）
//! - `validate_draft`：校验未保存草稿，只读不污染
//! - `compile_all`：CLI 全量（两阶段：注册全部 → 全量重检 + 继承环检测）

use crate::defs::{
    DefKind, DefTable, DefValue, RawDef, TableIndex, compile_bean, compile_enum, compile_record,
    compile_table,
};
use crate::diagnostic::Diagnostic;
use crate::types::{TypeInfo, TypeRef, TypeResolver};
use crate::value::DataContext;
use std::collections::{HashMap, HashSet, VecDeque};

/// 符号表：内存中的单一事实源。
#[derive(Debug, Default)]
pub struct SymbolTable {
    /// 原始定义（唯一事实源，重编译时从它重新编译）
    raws: HashMap<String, RawDef>,
    /// 编译缓存
    defs: HashMap<String, DefValue>,
    /// 每个定义的当前诊断
    diagnostics: HashMap<String, Vec<Diagnostic>>,
    /// 依赖出边：full_name -> 它引用的 full_name 集合
    deps: HashMap<String, Vec<String>>,
    /// 依赖入边：被引用者 -> 引用者集合（用于失效传播）
    reverse_deps: HashMap<String, HashSet<String>>,
    /// 最后一次操作实际重检的定义（测试断言增量正确性用）
    last_rechecked: Vec<String>,
}

impl SymbolTable {
    pub fn new() -> Self {
        Self::default()
    }

    // ========================================================================
    // 核心操作
    // ========================================================================

    /// 注册（创建）一个定义：查重 → 编译 → 插入 → 建依赖边。
    pub fn register(&mut self, def: &RawDef) -> Vec<Diagnostic> {
        let full = def.full_name();
        if self.defs.contains_key(&full) {
            return vec![Diagnostic::error(
                &full,
                format!("{} '{}' 已存在，不能重复注册", def.kind().label(), full),
            )];
        }
        let (value, deps, diags) = self.compile_one(def);
        self.raws.insert(full.clone(), def.clone());
        self.defs.insert(full.clone(), value);
        self.diagnostics.insert(full.clone(), diags.clone());
        self.set_deps(&full, deps);

        // 新注册的类型可能满足既有的"未解析"引用（如删除后重新注册），
        // 需重检依赖它的定义以消除残留诊断。全新类型无依赖者，此为无害空操作。
        let mut all = diags;
        all.extend(self.recheck_dependents(&full));
        all
    }

    /// 更新（编辑）一个定义：重编译 → 重建依赖边 → 反向重检所有依赖者。
    pub fn update(&mut self, def: &RawDef) -> Vec<Diagnostic> {
        let full = def.full_name();
        if !self.defs.contains_key(&full) {
            return vec![Diagnostic::error(
                &full,
                format!("{} '{}' 不存在，无法更新", def.kind().label(), full),
            )];
        }
        let (value, deps, diags) = self.compile_one(def);
        self.raws.insert(full.clone(), def.clone());
        self.defs.insert(full.clone(), value);
        self.diagnostics.insert(full.clone(), diags.clone());
        self.set_deps(&full, deps);

        let mut all = diags;
        all.extend(self.recheck_dependents(&full));
        all
    }

    /// 删除一个定义：移除 → 依赖者重检（立即产生"未解析类型"诊断）。
    pub fn remove(&mut self, full_name: &str) -> Vec<Diagnostic> {
        if !self.defs.contains_key(full_name) {
            return vec![Diagnostic::error(
                full_name,
                format!("定义 '{}' 不存在", full_name),
            )];
        }
        self.raws.remove(full_name);
        self.defs.remove(full_name);
        self.diagnostics.remove(full_name);
        self.clear_deps(full_name);
        self.recheck_dependents(full_name)
    }

    /// 校验未保存草稿：只读，不落盘、不污染符号表。
    pub fn validate_draft(&self, def: &RawDef) -> Vec<Diagnostic> {
        let (_, _, diags) = self.compile_one(def);
        diags
    }

    /// 全量编译（CLI 用）：清空 → 注册全部 → 全量重检 + 继承环检测。
    pub fn compile_all(&mut self, defs: &[RawDef]) -> Vec<Diagnostic> {
        self.clear();
        let mut dup_diags = Vec::new();

        // 第一遍：注册所有（容忍顺序，中间态的"未解析"诊断丢弃）
        for def in defs {
            let full = def.full_name();
            if self.defs.contains_key(&full) {
                dup_diags.push(Diagnostic::error(
                    &full,
                    format!("{} '{}' 重复定义", def.kind().label(), full),
                ));
                continue;
            }
            let (value, deps, _diags) = self.compile_one(def);
            self.raws.insert(full.clone(), def.clone());
            self.defs.insert(full.clone(), value);
            self.diagnostics.insert(full.clone(), Vec::new());
            self.set_deps(&full, deps);
        }

        // 第二遍：全量重检（此时所有定义就绪，依赖解析正确）
        let mut final_diags = self.recheck_all();
        final_diags.extend(self.check_inheritance_cycles());
        final_diags.extend(dup_diags);
        final_diags
    }

    // ========================================================================
    // 查询
    // ========================================================================

    pub fn has(&self, full_name: &str) -> bool {
        self.defs.contains_key(full_name)
    }

    pub fn kind_of(&self, full_name: &str) -> Option<DefKind> {
        self.defs.get(full_name).map(|v| v.kind())
    }

    /// Bean 的层级字段名（含父类，从根到自身）。非 Bean 返回 `None`。
    pub fn bean_field_names_of(&self, full_name: &str) -> Option<Vec<String>> {
        match self.defs.get(full_name) {
            Some(DefValue::Bean(b)) => Some(b.hierarchy_field_names.clone()),
            _ => None,
        }
    }

    /// 所有表的 full_name（排序）。
    pub fn table_names(&self) -> Vec<String> {
        let mut v: Vec<String> = self
            .defs
            .iter()
            .filter(|(_, d)| matches!(d, DefValue::Table(_)))
            .map(|(k, _)| k.clone())
            .collect();
        v.sort();
        v
    }

    /// 按 full_name 取表定义。
    pub fn get_table(&self, full_name: &str) -> Option<&DefTable> {
        match self.defs.get(full_name) {
            Some(DefValue::Table(t)) => Some(t),
            _ => None,
        }
    }

    pub fn total_count(&self) -> usize {
        self.defs.len()
    }

    pub fn enum_count(&self) -> usize {
        self.defs
            .values()
            .filter(|v| matches!(v, DefValue::Enum(_)))
            .count()
    }

    pub fn bean_count(&self) -> usize {
        self.defs
            .values()
            .filter(|v| matches!(v, DefValue::Bean(_)))
            .count()
    }

    pub fn table_count(&self) -> usize {
        self.defs
            .values()
            .filter(|v| matches!(v, DefValue::Table(_)))
            .count()
    }

    pub fn record_count(&self) -> usize {
        self.defs
            .values()
            .filter(|v| matches!(v, DefValue::Record(_)))
            .count()
    }

    /// 全部诊断（按 source 排序，输出稳定）。
    pub fn all_diagnostics(&self) -> Vec<Diagnostic> {
        let mut v: Vec<Diagnostic> = self.diagnostics.values().flatten().cloned().collect();
        v.sort_by(|a, b| (a.source.as_deref(), &a.message).cmp(&(b.source.as_deref(), &b.message)));
        v
    }

    /// 某定义的诊断。
    pub fn diagnostics_of(&self, full_name: &str) -> Option<&[Diagnostic]> {
        self.diagnostics.get(full_name).map(|v| v.as_slice())
    }

    /// 是否有任何错误。
    pub fn is_ok(&self) -> bool {
        self.diagnostics
            .values()
            .all(|v| v.iter().all(|d| !d.is_error()))
    }

    /// 最后一次操作实际重检的定义（增量正确性的观测窗口）。
    pub fn last_rechecked(&self) -> &[String] {
        &self.last_rechecked
    }

    // ========================================================================
    // 内部：编译 + 依赖图维护
    // ========================================================================

    /// 编译一个定义（用当前符号表作为类型解析器）。
    fn compile_one(&self, def: &RawDef) -> (DefValue, Vec<String>, Vec<Diagnostic>) {
        match def {
            RawDef::Enum(r) => {
                let (d, deps, diags) = compile_enum(r);
                (DefValue::Enum(d), deps, diags)
            }
            RawDef::Bean(r) => {
                let (d, deps, diags) = compile_bean(r, self);
                (DefValue::Bean(d), deps, diags)
            }
            RawDef::Table(r) => {
                let (d, deps, diags) = compile_table(r, self);
                (DefValue::Table(d), deps, diags)
            }
            RawDef::Record(r) => {
                let (d, deps, diags) = compile_record(r, self);
                (DefValue::Record(d), deps, diags)
            }
        }
    }

    /// 重建某定义的依赖边（先清旧出边，再建新出边）。
    fn set_deps(&mut self, full: &str, deps: Vec<String>) {
        // 去重
        let mut seen = HashSet::new();
        let deps: Vec<String> = deps
            .into_iter()
            .filter(|d| seen.insert(d.clone()))
            .collect();

        // 清旧出边
        if let Some(old) = self.deps.remove(full) {
            for dep in old {
                if let Some(s) = self.reverse_deps.get_mut(&dep) {
                    s.remove(full);
                    if s.is_empty() {
                        self.reverse_deps.remove(&dep);
                    }
                }
            }
        }

        // 建新出边
        self.deps.insert(full.to_string(), deps.clone());
        for dep in &deps {
            self.reverse_deps
                .entry(dep.clone())
                .or_default()
                .insert(full.to_string());
        }
    }

    /// 清除某定义的依赖出边。
    fn clear_deps(&mut self, full: &str) {
        if let Some(old) = self.deps.remove(full) {
            for dep in old {
                if let Some(s) = self.reverse_deps.get_mut(&dep) {
                    s.remove(full);
                    if s.is_empty() {
                        self.reverse_deps.remove(&dep);
                    }
                }
            }
        }
    }

    /// 重新编译一个定义（从 raws 克隆原始定义）。
    fn recompile_one(&mut self, full: &str) -> Option<Vec<Diagnostic>> {
        let raw = self.raws.get(full)?.clone();
        let (value, deps, diags) = self.compile_one(&raw);
        self.defs.insert(full.to_string(), value);
        self.diagnostics.insert(full.to_string(), diags.clone());
        self.set_deps(full, deps);
        Some(diags)
    }

    /// 反向失效传播：BFS 重检所有（传递）依赖 `changed` 的定义。
    fn recheck_dependents(&mut self, changed: &str) -> Vec<Diagnostic> {
        let mut result = Vec::new();
        let mut queue = VecDeque::new();
        let mut visited = HashSet::new();
        queue.push_back(changed.to_string());
        visited.insert(changed.to_string());

        let mut rechecked = Vec::new();
        while let Some(cur) = queue.pop_front() {
            let dependents: Vec<String> = self
                .reverse_deps
                .get(&cur)
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .collect();
            for dep in dependents {
                if !visited.insert(dep.clone()) {
                    continue;
                }
                if let Some(diags) = self.recompile_one(&dep) {
                    rechecked.push(dep.clone());
                    result.extend(diags);
                }
                queue.push_back(dep);
            }
        }
        self.last_rechecked = rechecked;
        result
    }

    /// 全量重检所有定义（compile_all 第二遍）。
    fn recheck_all(&mut self) -> Vec<Diagnostic> {
        let names: Vec<String> = self.raws.keys().cloned().collect();
        let mut all = Vec::new();
        for name in names {
            if let Some(diags) = self.recompile_one(&name) {
                all.extend(diags);
            }
        }
        all
    }

    /// 继承环检测（跨定义校验，单定义编译做不了）。
    fn check_inheritance_cycles(&self) -> Vec<Diagnostic> {
        let mut diags = Vec::new();
        for name in self.raws.keys() {
            if !matches!(self.defs.get(name), Some(DefValue::Bean(_))) {
                continue;
            }
            let mut visited = HashSet::new();
            visited.insert(name.clone());
            let mut cur = self.bean_parent(name);
            while let Some(p) = cur {
                if visited.contains(&p) {
                    diags.push(Diagnostic::error(
                        p.clone(),
                        format!("检测到 Bean 继承环，涉及 '{}'", p),
                    ));
                    break;
                }
                visited.insert(p.clone());
                cur = self.bean_parent(&p);
            }
        }
        diags
    }

    fn bean_parent(&self, full: &str) -> Option<String> {
        match self.defs.get(full) {
            Some(DefValue::Bean(b)) => b.parent.clone(),
            _ => None,
        }
    }

    fn clear(&mut self) {
        self.raws.clear();
        self.defs.clear();
        self.diagnostics.clear();
        self.deps.clear();
        self.reverse_deps.clear();
        self.last_rechecked.clear();
    }
}

// ========================================================================
// 作为类型解析器（供单定义编译查询）
// ========================================================================

impl TypeResolver for SymbolTable {
    fn resolve(&self, full_name: &str) -> Option<TypeRef> {
        match self.defs.get(full_name) {
            Some(DefValue::Enum(_)) => Some(TypeRef::Enum),
            Some(DefValue::Bean(_)) => Some(TypeRef::Bean),
            Some(DefValue::Record(_)) => Some(TypeRef::Record),
            _ => None,
        }
    }

    fn bean_field_names(&self, full_name: &str) -> Option<Vec<String>> {
        match self.defs.get(full_name) {
            Some(DefValue::Bean(b)) => Some(b.hierarchy_field_names.clone()),
            Some(DefValue::Record(r)) => {
                Some(r.fields.iter().map(|f| f.name.clone()).collect())
            }
            _ => None,
        }
    }

    fn bean_hierarchy_fields(&self, full_name: &str) -> Option<Vec<(String, TypeInfo)>> {
        match self.defs.get(full_name) {
            Some(DefValue::Bean(b)) => Some(
                b.hierarchy_fields
                    .iter()
                    .map(|f| (f.name.clone(), f.type_info.clone()))
                    .collect(),
            ),
            Some(DefValue::Record(r)) => Some(
                r.fields
                    .iter()
                    .map(|f| (f.name.clone(), f.type_info.clone()))
                    .collect(),
            ),
            _ => None,
        }
    }

    fn record_indexes(&self, full_name: &str) -> Option<Vec<TableIndex>> {
        match self.defs.get(full_name) {
            Some(DefValue::Record(r)) => Some(r.index.clone()),
            _ => None,
        }
    }
}

// ========================================================================
// 作为数据上下文（供数据加载/导出查询枚举值、Bean 字段）
// ========================================================================

impl DataContext for SymbolTable {
    fn enum_value(&self, enum_name: &str, value: &str) -> Option<i64> {
        match self.defs.get(enum_name) {
            Some(DefValue::Enum(e)) => {
                for item in &e.items {
                    if item.name == value || item.alias.as_deref() == Some(value) {
                        return Some(item.value);
                    }
                }
                // flag 组合表达式（Fire|Ice）：仅 flag 枚举允许；每段须是已定义项名/别名/整数
                if value.contains('|') {
                    if !e.is_flag {
                        return None;
                    }
                    let lookup = |name: &str| {
                        e.items
                            .iter()
                            .find(|it| it.name == name || it.alias.as_deref() == Some(name))
                            .map(|it| it.value)
                    };
                    return crate::defs::parse_flag_expr(value, &lookup).ok();
                }
                crate::defs::parse_int_literal(value).ok()
            }
            _ => None,
        }
    }

    fn bean_fields(&self, bean_name: &str) -> Option<Vec<String>> {
        match self.defs.get(bean_name) {
            Some(DefValue::Bean(b)) => {
                Some(b.hierarchy_fields.iter().map(|f| f.name.clone()).collect())
            }
            Some(DefValue::Record(r)) => {
                Some(r.fields.iter().map(|f| f.name.clone()).collect())
            }
            _ => None,
        }
    }

    fn bean_hierarchy_fields(&self, bean_name: &str) -> Option<Vec<(String, TypeInfo)>> {
        match self.defs.get(bean_name) {
            Some(DefValue::Bean(b)) => Some(
                b.hierarchy_fields
                    .iter()
                    .map(|f| (f.name.clone(), f.type_info.clone()))
                    .collect(),
            ),
            Some(DefValue::Record(r)) => Some(
                r.fields
                    .iter()
                    .map(|f| (f.name.clone(), f.type_info.clone()))
                    .collect(),
            ),
            _ => None,
        }
    }
}

// ========================================================================
// 单元测试
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::defs::{RawBean, RawEnum, RawEnumItem, RawField, RawRecord, RawTable};

    fn enum_def(name: &str, items: &[&str]) -> RawDef {
        RawDef::Enum(RawEnum {
            name: name.into(),
            items: items
                .iter()
                .map(|n| RawEnumItem {
                    name: (*n).into(),
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        })
    }

    fn bean_def(name: &str, fields: &[(&str, &str)]) -> RawDef {
        RawDef::Bean(RawBean {
            name: name.into(),
            fields: fields
                .iter()
                .map(|(n, t)| RawField {
                    name: (*n).into(),
                    r#type: (*t).into(),
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        })
    }

    fn record_def(name: &str, fields: &[(&str, &str)], index: Option<&str>) -> RawDef {
        RawDef::Record(RawRecord {
            name: name.into(),
            index: index.map(str::to_string),
            fields: fields
                .iter()
                .map(|(n, t)| RawField {
                    name: (*n).into(),
                    r#type: (*t).into(),
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        })
    }

    #[test]
    fn register_reports_unresolved_type() {
        let mut s = SymbolTable::new();
        // 引用不存在的枚举 Quality
        let diags = s.register(&bean_def("Item", &[("q", "Quality")]));
        assert!(
            diags
                .iter()
                .any(|d| d.message.contains("未定义的类型 'Quality'"))
        );
        assert!(!s.is_ok());
    }

    #[test]
    fn update_propagates_to_dependents_only() {
        let mut s = SymbolTable::new();
        s.register(&enum_def("Quality", &["A", "B"]));
        s.register(&bean_def("Item", &[("q", "Quality")]));
        s.register(&bean_def("Other", &[("x", "int")])); // 无关定义
        assert!(s.is_ok());

        // 修改 Quality（新增枚举项 C）→ 只有 Item 被重检，Other 不动
        s.update(&enum_def("Quality", &["A", "B", "C"]));
        let rechecked = s.last_rechecked();
        assert!(
            rechecked.contains(&"Item".to_string()),
            "Item 应被重检: {:?}",
            rechecked
        );
        assert!(
            !rechecked.contains(&"Other".to_string()),
            "Other 不应被重检: {:?}",
            rechecked
        );
        assert!(s.is_ok());
    }

    #[test]
    fn remove_invalidates_dependents() {
        let mut s = SymbolTable::new();
        s.register(&enum_def("Quality", &["A", "B"]));
        s.register(&bean_def("Item", &[("q", "Quality")]));
        assert!(s.is_ok());

        // 删除 Quality → Item 立即变"未解析类型"
        let diags = s.remove("Quality");
        assert!(
            diags
                .iter()
                .any(|d| d.message.contains("未定义的类型 'Quality'"))
        );
        assert!(!s.is_ok());
        assert_eq!(s.total_count(), 1); // 只剩 Item
    }

    #[test]
    fn remove_then_register_recovers() {
        let mut s = SymbolTable::new();
        s.register(&enum_def("Quality", &["A"]));
        s.register(&bean_def("Item", &[("q", "Quality")]));
        s.remove("Quality");
        assert!(!s.is_ok());

        // 重新注册 Quality → Item 重检后错误消失
        s.register(&enum_def("Quality", &["A"]));
        assert!(s.is_ok());
    }

    #[test]
    fn validate_draft_does_not_pollute() {
        let mut s = SymbolTable::new();
        s.register(&enum_def("Quality", &["A"]));
        let before_count = s.total_count();

        // 校验一个引用不存在类型的草稿：返回诊断，但不注册、不改变符号表
        let diags = s.validate_draft(&bean_def("Draft", &[("q", "Nope")]));
        assert!(
            diags
                .iter()
                .any(|d| d.message.contains("未定义的类型 'Nope'"))
        );
        assert_eq!(s.total_count(), before_count);
        assert!(!s.has("Draft"));
        assert!(s.is_ok()); // 原有定义不受影响
    }

    #[test]
    fn compile_all_two_phase_resolves_order() {
        let mut s = SymbolTable::new();
        // Bean 引用枚举，但 Bean 排在枚举前面（顺序无关）
        let defs = vec![
            bean_def("Item", &[("q", "Quality")]),
            enum_def("Quality", &["A"]),
        ];
        let diags = s.compile_all(&defs);
        assert!(diags.is_empty(), "两阶段后不应有未解析: {:?}", diags);
        assert!(s.is_ok());
        assert_eq!(s.total_count(), 2);
    }

    #[test]
    fn compile_all_reports_duplicate() {
        let mut s = SymbolTable::new();
        let defs = vec![enum_def("E", &["A"]), enum_def("E", &["B"])];
        let diags = s.compile_all(&defs);
        assert!(diags.iter().any(|d| d.message.contains("重复定义")));
    }

    #[test]
    fn inheritance_cycle_detected() {
        let mut s = SymbolTable::new();
        let a = RawDef::Bean(RawBean {
            name: "A".into(),
            parent: Some("B".into()),
            ..Default::default()
        });
        let b = RawDef::Bean(RawBean {
            name: "B".into(),
            parent: Some("A".into()),
            ..Default::default()
        });
        let diags = s.compile_all(&[a, b]);
        assert!(
            diags.iter().any(|d| d.message.contains("继承环")),
            "应检测到继承环: {:?}",
            diags
        );
    }

    #[test]
    fn table_index_validation_against_bean() {
        let mut s = SymbolTable::new();
        s.register(&bean_def("Item", &[("id", "int"), ("name", "string")]));
        // 索引列不存在 → 诊断
        let diags = s.register(&RawDef::Table(RawTable {
            name: "TbItem".into(),
            value_type: "Item".into(),
            mode: Some("map".into()),
            index: Some("missing".into()),
            ..Default::default()
        }));
        assert!(diags.iter().any(|d| d.message.contains("索引列 'missing'")));
    }

    #[test]
    fn record_as_table_value_type_and_auto_index() {
        let mut s = SymbolTable::new();
        let record = record_def("ItemRec", &[("id", "int"), ("name", "string")], Some("id"));
        let table = RawDef::Table(RawTable {
            name: "TbItemRec".into(),
            value_type: "ItemRec".into(),
            mode: Some("list".into()),
            ..Default::default()
        });
        let diags = s.compile_all(&[record, table]);
        assert!(diags.is_empty(), "表编译不应有诊断: {:?}", diags);
        assert!(s.is_ok());
        assert_eq!(s.resolve("ItemRec"), Some(TypeRef::Record));
        assert_eq!(
            s.bean_field_names("ItemRec"),
            Some(vec!["id".to_string(), "name".to_string()])
        );
        assert_eq!(
            s.get_table("TbItemRec").unwrap().index,
            vec![TableIndex {
                columns: vec!["id".to_string()]
            }]
        );
    }
}
