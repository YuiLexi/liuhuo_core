# LiuHuo Schema 编译管线：Raw → Def 代码逻辑详解

> 对应源码：`liuhuo_core/src/defs.rs`（993 行）、`src/symbol.rs`（743 行）、`src/types.rs`（564 行）
> 本文基于 test 分支当前 HEAD（含 5 项新校验功能）

---

## 一、总览：两层模型 + 一个符号表

LiuHuo 的 schema 体系把"定义"分成两层：

```
Raw 层（defs.rs RawXxx）          Def 层（defs.rs DefXxx）
磁盘 JSON 的哑数据      ──编译──▶  内存语义模型
serde 直接反序列化                 可校验、可查层级字段、带类型信息
        │                                ▲
        │  唯一事实源（raws）             │ 编译缓存（defs）
        └──────── SymbolTable ───────────┘
                     │
              依赖图（deps / reverse_deps）
              增量失效传播（BFS 反向重检）
```

**核心设计决策**（defs.rs 头注释原文的意思）：

1. **单定义编译是增量编译的基石**：每个编译函数返回三元组 `(编译结果, 依赖列表, 诊断)`。依赖列表告诉符号表"我引用了谁"，用于建依赖图。
2. **"引用了不存在的类型"不是解析失败，而是诊断**：`parse_type` 查不到的类型返回 `TypeKind::Unresolved(name)`，编译继续、报一条 error。这样定义可以按任意顺序创建/编辑，缺依赖时系统不崩，且依赖补上后重检自动消错。

---

## 二、Raw 层：磁盘 JSON 的直接映射

四种定义共用同一套 Raw 结构风格（全部字段 `#[serde(default)]`，宽松反序列化）：

```rust
pub struct RawEnum   { name, module, comment, alias, is_flag, items: Vec<RawEnumItem>, ... }
pub struct RawEnumItem { name, value: Option<String>, alias, comment, properties }

pub struct RawBean   { name, module, comment, alias, sep, is_value,
                       parent: Option<String>,          // 继承
                       fields: Vec<RawField>, ... }

pub struct RawField  { name, r#type: String,             // 类型是字符串！"int(nonneg)"、
                       comment, groups, properties }     // "list<ref<game.TbItem>>(size=[1,3])"

pub struct RawTable  { name, module, comment,
                       index: Option<String>,            // "id" 或联合 "a+b"
                       value_type: String,               // 记录类型（bean 或 record）
                       mode: Option<String>,             // one / map / list
                       input: Vec<String>, ... }

pub struct RawRecord { name, module, comment,
                       fields: Vec<RawField>,
                       index: Option<String>, ... }      // 无继承的轻量记录类型

pub enum RawDef { Enum(RawEnum), Bean(RawBean), Table(RawTable), Record(RawRecord) }
```

关键点：

- **Raw 里没有类型信息**——字段的 `type` 就是一坨字符串。类型解析发生在编译期。
- **full_name 规则**：`full_name(module, name)` = `module.name`，空 module 时裸名。所有跨定义引用都用 full_name。
- RawDef 的 serde 是**外部标签**格式：`{"Bean": {...}}`。

## 三、类型层（types.rs）：字符串 → TypeKind

`parse_type("list<ref<game.TbItem>>(size=[1,3])?", resolver)` 的处理顺序：

```
1. extract_trailing_paren_tags —— 从尾部剥出 "(size=[1,3])" → tags
   （括号配对从右往左扫，depth 归零处切开）
2. strip_suffix('?') —— 可空标记
3. parse_type_expr —— 解析类型表达式本身：
   基础类型（bool/i8..u64/f32/f64/string/text/datetime + 别名）
   容器：array<T> / list<T> / set<T> / map<K,V>（递归解析元素）
   引用：ref<TbX> → TypeKind::Ref(full_name)
   其它名字 → 查 resolver：
       Some(Enum)   → TypeKind::Enum(n)
       Some(Bean)   → TypeKind::Bean(n)
       Some(Record) → TypeKind::Bean(n)     // record 按无继承 bean 对待
       None         → TypeKind::Unresolved(n) ← 关键：不报错，产出"未解析"类型
```

产物 `TypeInfo { kind, nullable, tags }`——tags 就是校验标签的载体（range/nonneg/size/path 都挂在这里）。`collect_refs()` 递归收集容器内所有引用名（依赖图数据源）；`unresolved_refs()` 收集所有未解析名（编译诊断数据源）。

## 四、Def 层：编译后的语义模型

| Raw | Def | 编译增益 |
|---|---|---|
| RawEnum（value 是字符串） | DefEnum（value 是 **i64**，已解析） | 自动编号、flag 位、组合表达式求值、i32 域检查 |
| RawBean（parent 是字符串） | DefBean（parent 是 full_name + **hierarchy_fields**） | 层级字段合并（含父类全部字段）、shadow 冲突检测 |
| RawTable（index 是字符串） | DefTable（index 是 `Vec<TableIndex>`） | 模式规范化、record 索引自动继承、索引列存在性校验 |
| RawRecord | DefRecord（index 已解析校验） | 索引列存在性校验 |

DefBean 里最值钱的是两个层级字段缓存：

```rust
pub hierarchy_field_names: Vec<String>,   // 含父类字段名（根→自身），查冲突用
pub hierarchy_fields:     Vec<DefField>,  // 含父类完整字段（名+类型），数据加载用
```

数据加载/校验/导出全用 `hierarchy_fields`——继承合并只算一次，后续 O(1) 查询。

## 五、四个编译函数的逻辑

统一签名模式：`fn compile_xxx(raw, resolver) -> (DefXxx, Vec<String>/*deps*/, Vec<Diagnostic>)`

### 5.1 compile_enum —— 无依赖，纯自足编译

```
遍历 items：
  ① 重名检查（HashSet）
  ② 值解析，三分支：
     显式整数字面量 → parse_int_literal（支持 0x/0b/_/负号）
     名称/组合表达式 → parse_flag_expr（Fire|Ice 按位或，查已编译的前置项）
     无值 → 自动递增：普通枚举 value+1，flag 枚举 value<<1
  ③ next 推进（显式值也推进游标）
PostCompile：全部项做 i32 值域检查
返回 deps = 空（枚举不依赖任何类型）
```

### 5.2 compile_bean —— 依赖最重的一个

```
① 父类解析：resolver.resolve(parent)
   Bean   → 记依赖边，parent = Some
   Enum   → 诊断"枚举不能作父类"
   Record → 记依赖 + 诊断"record 不能作父类"
   None   → 记依赖 + 诊断"父类不存在"     ← 注意：不存在也建依赖边！
                                              父类后来被创建时，反向重检自动消错
② 逐字段 parse_type：
   成功 → collect_refs 进 deps；unresolved_refs 转诊断；产出 DefField
   失败 → 诊断"类型无效"，字段跳过
③ 层级字段合并：父类的 hierarchy_fields（resolver 查询）
   + shadow 检查：自己字段名撞父类字段名 → 诊断"字段冲突"
   产出 hierarchy_field_names / hierarchy_fields（父类字段在前）
```

### 5.3 compile_table

```
① mode 规范化：None/""/one → One；map → Map；list → List；其它 → 诊断
② value_type 解析：Bean/Record → 依赖边；Enum → 诊断；None → 依赖边 + 诊断
③ 索引：parse_index 解析 "a+b" 联合索引
   + resolver.record_indexes() —— record 的索引自动继承进表（新功能）：
     record 自带的 index 若表未显式声明则自动加入 → 自动获得唯一性校验
④ map 模式空索引兜底：取 value_type 第一个字段当索引
⑤ 索引列校验：每列必须存在于 value_type 的字段中（bean 用层级字段）
```

### 5.4 compile_record —— compile_bean 的无继承简化版

```
索引解析 + 索引列存在性校验（对自身字段）
逐字段 parse_type（同 bean ②）
无父类、无层级合并、无 shadow 检查
```

## 六、SymbolTable：符号表 + 依赖图 + 增量（symbol.rs）

### 6.1 数据结构

```rust
pub struct SymbolTable {
    raws:   HashMap<String, RawDef>,              // 唯一事实源
    defs:   HashMap<String, DefValue>,            // 编译缓存（可随时从 raws 重算）
    diagnostics: HashMap<String, Vec<Diagnostic>>,
    deps:         HashMap<String, Vec<String>>,   // 出边：我引用了谁
    reverse_deps: HashMap<String, HashSet<String>>,// 入边：谁引用了我（失效传播用）
    last_rechecked: Vec<String>,                   // 观测窗口：上次操作实际重检了谁
}
```

**双重角色**：SymbolTable 自己实现 `TypeResolver`——`resolve()` 查自己的 defs 缓存回答"这个 full_name 是什么种类"。也就是说：**符号表 = 类型解析上下文 = 编译缓存 = 依赖图**，四位一体，编译函数通过 resolver 参数与它解耦（测试时可注入 mock resolver）。

### 6.2 五个核心操作

| 操作 | 逻辑 | 语义 |
|---|---|---|
| `register` | 查重 → compile_one → 插入 raws/defs → set_deps → **recheck_dependents** | 创建即编译。重检依赖者的原因：新类型可能满足别人的 Unresolved 引用（比如删除后又重建） |
| `update` | 存在性检查 → compile_one → 覆盖 → set_deps（先清旧出边再建新）→ recheck_dependents | 编辑即重编译 + 反向传播 |
| `remove` | 删三表 → clear_deps → recheck_dependents | 删除即失效，依赖者立即出"未解析"诊断 |
| `validate_draft` | `&self` 只读调 compile_one，**不落任何表** | 编辑器实时校验草稿（就是前端 validate_cell 想要的通道） |
| `compile_all` | clear → 第一遍注册全部（**丢弃中间态诊断**）→ recheck_all + 继承环检测 | CLI/保存时全量编译。两遍是为了容忍定义文件间的乱序引用 |

### 6.3 失效传播：recheck_dependents（BFS）

```
queue = [changed]; visited = {changed}
while let cur = queue.pop_front():
    for dep in reverse_deps[cur]:        // 谁引用了 cur
        if visited.insert(dep):
            recompile_one(dep)           // 从 raws 克隆 raw → 重新编译 → 覆盖缓存/诊断/依赖边
            rechecked.push(dep)          // 记入 last_rechecked
            queue.push_back(dep)         // 传递传播：依赖者的依赖者也要重检
```

改一个被广泛引用的 Bean（如 BaseReward），所有子类、以它为 value_type 的表、引用这些表的 ref 字段所在 bean……整条链自动重编译。`last_rechecked` 是给测试断言增量正确性用的观测窗口（K7 组就用它验证"无关定义不被重检"）。

### 6.4 继承环检测（check_inheritance_cycles）

单定义编译看不见跨定义的环（A→B→A 各自编译都合法），所以 compile_all 第二遍后对每个 Bean 沿 parent 链走，visited 命中即报"继承环"。这是唯一必须在符号表层面做的跨定义校验。

## 七、一条完整链路示例

```
用户创建 {"name":"ItemReward","parent":"game.BaseReward",
          "fields":[{"name":"count","type":"int(nonneg)"}]}

register(RawDef::Bean)
  └─ compile_bean(raw, &symbol_table)
       ├─ resolve("game.BaseReward") → Some(Bean) → deps=["game.BaseReward"]
       ├─ parse_type("int(nonneg)") → TypeInfo{Int32, tags:{nonneg:"true"}}
       └─ bean_hierarchy_fields("game.BaseReward") → [id, name...]（父类已编译过）
          → hierarchy_fields = [id, name..., count]，无 shadow
  └─ set_deps: deps["game.ItemReward"]=["game.BaseReward"]
               reverse_deps["game.BaseReward"] += "game.ItemReward"
  └─ recheck_dependents("game.ItemReward") → 此刻无人引用它，空操作
返回 []（零诊断）

之后用户 update BaseReward 加了字段 "name"：
  └─ recheck_dependents("game.BaseReward")
       └─ ItemReward 被重编译 → hierarchy_fields 自动包含新 name 字段
          （诊断/缓存与数据校验全部跟上，无需手动刷新）
```

## 八、设计要点小结

1. **Raw 是唯一事实源，Def 是缓存**——任何 Def 都能从 Raw + resolver 确定性重算，这是增量正确性的根基（重编译永远从 raw 出发，不存在缓存漂移）。
2. **Unresolved 不是错误路径是正常状态**——乱序创建、删除依赖、循环补全都自然工作，编辑器体验的关键。
3. **编译函数 = 纯函数 + resolver 注入**——不持有状态，可单测（EmptyResolver 即纯语法测试），也是 validate_draft 免费拿到的原因。
4. **依赖边在诊断时也建**——"父类不存在"依然 push deps，让未来的反向重检有机会消错。
5. **四位一体的 SymbolTable**——解析器/缓存/索引/依赖图不分裂，编译函数只通过 TypeResolver trait 与它耦合。
