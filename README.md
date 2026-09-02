# LiuHuoCore（流火核心库）

流火（LiuHuo）是以 [Luban](https://github.com/focus-creative-games/luban) 的设计理念为原型、使用 Rust 从零实现的游戏配表工具核心库。
具备完备的类型系统、多种数据文件加载方式、校验 / 导出 / 公式 / 代码生成 / 本地化全管线，并通过 trait + 注册表设计支持轻松二次开发。

> 本仓库是 `liuhuo_core` 的唯一开发仓库（此前位于 `E:\Projects\IT_信息技术\GT_游戏技术\ATD_自动化工具开发\LiuHuoCore`，已于 2026-09 迁移至此）。
> `main` 为稳定分支，`develop` 为开发分支，`test` 为独立测试环境分支。

## 设计哲学（借鉴 Luban，独立实现）

| 维度 | Luban（C#） | LiuHuo（Rust） |
|---|---|---|
| 定义文件 | XML | JSON（每个定义一个文件） |
| 类型系统 | 类继承 + 反射 | `enum TypeKind` + trait 注册表 |
| 扩展点 | `[SchemaLoader]` 反射特性 | trait + Registry + `register()` |
| 编译 | 每次全量三阶段 | **符号表 + 依赖图增量编译** |
| 数据值 | DInt/DBean 类层次 | `DType` 枚举（无装箱、无反射） |

```
Raw 层（磁盘文件的哑数据）              Def 层（解析完引用、带语义的模型）
┌──────────────────────┐    from_raw   ┌──────────────────────────────┐
│ RawEnum / RawBean    │ ────────────▶ │ DefEnum / DefBean / DefTable │
│ RawTable / RawRecord │    注册+查重   │ DefRecord / DefField         │
└──────────────────────┘               └──────────────────────────────┘
                                              │ 三阶段编译
                                              ▼
                                  PreCompile → Compile → PostCompile
```

### 三阶段编译

| 阶段 | 职责 |
|---|---|
| **PreCompile** | 全名查重（含大小写冲突）、枚举项查重、分组引用校验、Bean 继承体系建立（父类解析 / 循环检测 / 层级字段收集） |
| **Compile** | 枚举值解析（自动递增 / 位递增 / 名称引用）、字段类型字符串解析、表模式与索引编译 |
| **PostCompile** | 枚举值 i32 值域检查、字段校验器执行（range / not-empty 等） |

### 类型系统

- 基础类型：`bool i8 i16 i32 i64 u8 u16 u32 u64 f32 f64 string text datetime`
- 别名：`int→i32` `long→i64` `short→i16` `byte/uint8→u8` `float→f32` `double→f64` `time→datetime`
- 容器：`list<T> set<T> array<T> map<K,V>`（支持嵌套与空格）
- 可空后缀 `?`、非默认值后缀 `!`（容器元素禁止可空）
- 标签：`int(range=[1,100])`，解析出的标签并入字段 tags
- 自定义类型：模块感知解析（`module.name` 优先，其次裸名）；语法合法但未定义的类型解析为 `Unresolved`，作为语义错误在编译阶段诊断 —— 这是增量编译的关键

### 符号表 + 增量编译（核心超越点）

符号表 `SymbolTable` 是内存中的单一事实源，编译产物只是缓存：

- `raws: HashMap<FullName, RawDef>` —— 唯一事实源（full_name `String` 键）
- `defs: HashMap<FullName, DefValue>` —— 编译缓存
- `deps` / `reverse_deps` —— 依赖图，变更沿反向边 BFS 失效传播
- 五个操作：`register`（创建即编译）/ `update`（编辑即重编译 + 反向失效）/ `remove`（删除即失效）/ `validate_draft`（只读草稿校验）/ `compile_all`（CLI 全量，两阶段）
- 「动态向编译后的 Schema 添加类型」= 一次 `insert`，不存在冻结的程序集

### 模块地图（16 模块）

| 模块 | 职责 |
|---|---|
| `types` | 类型系统 + 类型串解析器 |
| `defs` | Raw / Def 两层 + 单定义三阶段编译 |
| `symbol` | 符号表 + 依赖图 + 增量编译 |
| `diagnostic` | GUI 导向的诊断收集（错误不中断，全部收集） |
| `value` / `data` | `DType` 数据值 + JSON 数据加载（按 TypeInfo 类型指导解码） |
| `text_data` | 自定义文本格式数据加载（git 友好，一行一记录） |
| `validate` | 字段值校验器 + 表级校验器（含跨表外键两阶段校验） |
| `export` | 数据导出 / JSON Schema 导出 |
| `formula` | 公式引擎（computed 列导出时物化，公式不落数据表） |
| `codegen` | 代码生成（C# / TypeScript / Rust 目标） |
| `l10n` | 本地化文本提取 |
| `config` | `liuhuo.config.yaml`（分组 / 全局参数 / 导出配置 / 标签过滤） |
| `project` | 项目工程化（创建 / 文件树 / 编译缓存） |

### 扩展点（抽象定义 + 多实现）

所有扩展点均为 trait + 注册表，`register()` 注入自定义实现，核心管线零改动：

| Trait | 职责 | 内置实现 |
|---|---|---|
| `IDataLoader` | 数据文件 → 表数据 | `JsonDataLoader`、`TextDataLoader` |
| `IDataValidator` / `ITableValidator` | 字段值 / 表级校验 | `RangeValidator`、`UniqueKeyValidator` 等 |
| `IExporter` | 导出产物 | `JsonDataExporter`、`JsonSchemaExporter` |
| `ICodeGenerator` | 代码生成 | `CsCodeGenerator`、`TsCodeGenerator`、`RustCodeGenerator` |

## 全管线

```
schema（enums/beans/records/tables/*.json）
      │ register → 三阶段编译（增量 or 全量）
      ▼
  DefAssembly
      │ 数据加载（JSON / 文本格式，解析即校验）
      ▼
  TableData（DType）──► 校验（字段值 + 表级 + 跨表外键）
      │ 公式物化（computed 列）──► 导出（json / json-schema）
      │ 代码生成（cs / ts / rust）──► 本地化提取
      ▼
  产物目录
```

诊断哲学：GUI 工具收集**所有**错误而非快速失败 —— 加载器返回 `(records, diagnostics)`，单条记录失败不丢弃其余好记录；错误定位格式 `表名[行N].字段`。

## 快速上手

```rust
use liuhuo_core::*;

// 全量编译（CLI 场景）
let outcome = compile_project(&project_dir)?;
for diag in &outcome.diagnostics {
    println!("{}", diag); // [error] 表名[行2].字段: 消息
}

// 增量编译（GUI 场景）
let mut sym = SymbolTable::new();
sym.register(raw_bean)?;          // 创建即编译，引用缺失立即诊断
sym.update(raw_enum)?;            // 编辑即重编译 + 反向失效依赖者
sym.remove("item.Quality")?;      // 删除即失效
let diags = sym.validate_draft(&draft)?; // 未保存草稿校验，不污染符号表
```

## 构建与测试

```bash
cargo check
cargo test          # 62 个测试（单元 + 集成）
```

独立的端到端测试脚本（不使用 Rust `#[test]` 框架，与源码隔离）见 `test` 分支的 `test_scripts/` 目录。

## 路线图

- [x] 类型系统 + 类型字符串解析
- [x] Raw → Def 两阶段 + 三阶段编译管线
- [x] Bean 继承体系（父类 / 子类 / 层级字段 / 循环检测）
- [x] 表编译（one / map / list 模式、联合 / 多键索引）
- [x] 符号表 + 依赖图增量编译（创建即校验、反向失效、恢复）
- [x] 数据值系统（DType）+ JSON / 文本数据加载
- [x] 校验闭环（字段值 + 表级 + 跨表外键）
- [x] 数据导出 + JSON Schema 导出
- [x] 公式引擎（computed 列 / apply_formula 固化）
- [x] 代码生成（C# / TS / Rust）
- [x] 本地化提取
- [ ] Excel / CSV 数据加载器
- [ ] Luban XML / Excel schema 兼容导入
- [ ] 可视化 UI（Tauri 2 桌面应用）

## 相关文档

- [docs/设计文档.md](docs/设计文档.md) —— 整体架构与增量编译落地设计
