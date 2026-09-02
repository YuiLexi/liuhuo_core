# liuhuo_core 独立测试环境

**分支约定：`develop` 只管开发代码；`test` 只用于测试 develop 的内容。**

工作流：develop 上完成开发并提交 → 在 test 分支上 `git merge develop` 同步最新代码 →
运行 `bash test_scripts/run_all.sh` 对 develop 的成果做端到端验证 → 测试不通过则回 develop 修复，再同步重测。

本目录是与源码**完全隔离**的端到端测试环境 —— 不使用 Rust `#[test]` 测试框架，
而是独立可执行脚本 + 自带测试数据。源码分支（develop/main）不包含本目录。

## 为什么独立

- 测试环境与源代码物理隔离：测试数据、测试工程不进入发布产物
- 脚本以「真实用户操作」视角跑完整管线（建项目 → 写定义 → 写数据 → 编译 → 校验 → 导出 → 代码生成），
  断言的是端到端行为而非内部函数
- 可以脱离 cargo test 单独运行、单独演化（如未来接 Excel 样例表）

## 目录结构

```
test_scripts/
├── README.md            本说明
├── run_all.sh           一键跑全部测试脚本
├── e2e_pipeline.rs      端到端管线测试（独立 bin，引用核心库）
├── projects/            测试用配表工程（脚本运行时生成/复用）
└── out/                 测试产物输出（导出数据/生成代码）
```

## 运行方式

```bash
# 在仓库根目录（test 分支）
bash test_scripts/run_all.sh
```

脚本通过 `cargo run --example` 风格（`rustc` 直接编译独立 rs 文件，
链接 `target/debug` 下的核心库 rlib）运行，不污染 `tests/` 目录。
