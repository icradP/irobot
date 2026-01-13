# MCP 项目集合（简要说明）

本仓库包含 4 个相互独立的 Rust 子项目，围绕 MCP（Model Context Protocol）、LLM 本地推理与控制台交互构建。每个项目均可在其目录中单独构建与运行。

## 子项目概览

- robot_core  
  - 机器人核心框架：会话管理、意图/路由、工作流引擎、MCP 客户端与工具集、LLM 适配（含本地 LM Studio 适配示例）、Web/TCP 控制台等。  
  - 代码入口：[main.rs](robot_core/src/main.rs)，工程配置：[Cargo.toml](robot_core/Cargo.toml)  
  - 运行（从仓库根目录执行）：
    ```bash
    cd robot_core
    cargo run
    ```

- robot_mcp_server  
  - MCP 服务器示例，提供工具如：天气、当前时间、Echo、求和、FFprobe、Profile、Chat 等；外部工具配置位于 `config/external.toml`。  
  - 代码入口：[main.rs](robot_mcp_server/src/main.rs)，工程配置：[Cargo.toml](robot_mcp_server/Cargo.toml)  
  - 运行（从仓库根目录执行）：
    ```bash
    cd robot_mcp_server
    cargo run
    ```

- robot_candle  
  - 使用 Candle 进行本地模型推理（例如 Qwen3），包含简单的配置与实用工具；默认配置位于 `config.json`。  
  - 代码入口：[main.rs](robot_candle/src/main.rs)，工程配置：[Cargo.toml](robot_candle/Cargo.toml)  
  - 运行（从仓库根目录执行）：
    ```bash
    cd robot_candle
    cargo run
    ```

- tcp_terminal  
  - 轻量级 TCP 终端/控制台示例。  
  - 代码入口：[main.rs](tcp_terminal/src/main.rs)，工程配置：[Cargo.toml](tcp_terminal/Cargo.toml)  
  - 运行（从仓库根目录执行）：
    ```bash
    cd tcp_terminal
    cargo run
    ```

## 环境与依赖

- Rust 稳定版（建议使用最新 stable toolchain）。  
- 每个子项目均为独立 Cargo 工程，互不依赖，可分别构建与运行。  
- 如需本地 LLM 推理或外部工具适配，请按各项目内的配置文件与代码注释进行相应设置（例如 robot_mcp_server 的 `config/external.toml`）。

## 开发提示

- 修改或扩展工具：参考 robot_mcp_server 的 `src/tools/` 目录结构与实现。  
- 扩展会话/工作流：参考 robot_core 的 `src/core/` 与 `src/workflow_engine.rs` 等模块。  
- 本地推理与模型配置：参考 robot_candle 的 `src/` 与 `config.json`。  
- 控制台/终端交互：参考 robot_core 的 Web/TCP 控制台与 tcp_terminal 的示例。
