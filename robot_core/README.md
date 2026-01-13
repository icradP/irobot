# robot_core（简要说明）

机器人核心框架：提供会话管理、意图识别与路由、工作流引擎、MCP 客户端与工具集、LLM 适配（含 LM Studio 示例）以及 Web/TCP 控制台等能力。

## 位置
- 入口代码： [src/main.rs](src/main.rs)
- 工程配置： [Cargo.toml](Cargo.toml)

## 运行
```bash
cargo run
```

## 提示
- 控制台相关实现位于 `src/tentacles/`。  
- 工作流与核心模块位于 `src/core/` 与 `src/workflow_engine.rs`。  
- LLM 适配位于 `src/llm/`。  
- 输入输出的特化实现触手位于 `src/tentacles/`。目前实现了 Web/TCP 控制台。

