# robot_core

robot_core 是 Robot 的核心运行时，负责把多种输入源聚合成统一事件流，基于 LLM 生成工作流计划，并在会话上下文中执行步骤（包含对 MCP 工具的调用与参数引导），再将输出路由到对应的输出端。

## 模块分层

- **core**：调度与会话运行时（事件路由、会话管理、工作流执行）
- **llm**：LLM 适配层（当前对接 LMStudio）
- **mcp**：MCP 客户端抽象与 rmcp 实现（工具发现、调用、引导）
- **workflow_steps**：工作流步骤与参数解析器（LLM 参数抽取）
- **tentacles**：具体输入/输出“触手”（当前为 Web 控制台）
- **utils**：事件结构、上下文、全局 event/output bus 与消费标记

入口文件：
- 二进制入口：[main.rs](src/main.rs)
- 库入口：[lib.rs](src/lib.rs)

## 核心数据流

1. **输入产生 InputEvent**
   - 输入源实现 `InputHandler`，产出 [InputEvent](src/utils/mod.rs#L9-L16)
   - 当前 Web 输入由 [WebInput](src/tentacles/web_console.rs#L81-L156) 提供

2. **RobotCore 分发到 SessionManager**
   - [RobotCore::run_once](src/core/mod.rs#L144-L160) 从输入 channel 取事件
   - 事件交给 [SessionManager::dispatch](src/core/session.rs#L206-L271)

3. **按 session_id 路由到会话执行**
   - `session_id` 来自 `InputEvent.session_id`，缺省为 `source`
   - 每个 session 对应一个 actor（tokio task），由 [RobotSession](src/core/session.rs#L26-L173) 处理
   - Web 来源会包一层 [WebSession](src/core/session.rs#L248-L253)（用于 Web 特定行为）

4. **DecisionEngine 生成 WorkflowPlan**
   - 决策引擎接口：[DecisionEngine](src/core/decision_engine.rs#L10-L13)
   - 当前实现：[LLMDecisionEngine](src/core/decision_engine.rs#L34-L160)
   - 会先通过 MCP `list_tools()` 获取工具列表，交给 LLM 选择步骤

5. **执行 workflow_steps**
   - `plan.steps` 是一组 [StepSpec](src/utils/mod.rs#L81-L87)
   - `Tool` 类型的 StepSpec 会落到 [McpToolStep](src/workflow_steps/mod.rs#L237-L271)，通过 MCP 调用工具

6. **输出 OutputEvent 与路由**
   - 步骤可产生 [OutputEvent](src/utils/mod.rs#L18-L25)
   - session 会根据 [EventRouter](src/core/router.rs) 决定发往哪些输出 handler
   - 输出 handler 接口：[OutputHandler](src/core/output_handler.rs)
   - 当前 Web 输出为 [WebOutput](src/tentacles/web_console.rs#L173-L260)

## MCP 交互与参数引导（elicitation）

### 1) MCP 客户端抽象

- 接口：[MCPClient](src/mcp/client.rs#L5-L14)
- 当前实现：[RmcpStdIoClient](src/mcp/rmcp_client.rs#L18-L25)

支持能力：
- `list_tools()`：发现 MCP server 暴露的工具元数据
- `call()`：调用工具
- `required_fields()` / `tool_schema()`：用于参数抽取与前端预览

### 2) 参数引导的事件闭环

rmcp 的 `create_elicitation` 回调会：
- 把 server 侧引导消息与 schema 发到 `output_bus`（source=mcp）
- 订阅 `event_bus` 等待用户在同一 session_id 下回传输入
- 尝试把输入解析成 JSON；若不是 JSON，用 LLM 按 schema 转为 JSON
- 将该输入事件标记为 consumed，避免重复被 session 主流程处理（见 [mark_event_consumed](src/utils/mod.rs#L111-L124) 与 [RobotSession::handle_input](src/core/session.rs#L63-L70)）

实现位置：[RobotClientHandler::create_elicitation](src/mcp/rmcp_client.rs#L49-L206)

### 3) 断线不致命 + 下次调用自动恢复

`RmcpStdIoClient` 采用“延迟连接 + 按需重连”：
- 构造时不连接 MCP server
- 在第一次 `call/list_tools/...` 时建立连接
- 若调用返回连接类错误，会清空连接并重连，然后重试一次

实现位置：
- [ensure_connected / connect / with_service_retry](src/mcp/rmcp_client.rs#L236-L370)

## 参数解析器（LLM Parameter Resolver）

当步骤是 “调用工具” 时，如果 StepSpec 携带的 args 不是对象：
- 会读取 tool schema + required 字段
- 调用 LLM 将用户自然语言转成 JSON 参数
- 对缺失的必填字段输出 `null`，让 MCP server 侧触发引导（而不是客户端猜测）

实现位置：[LlmParameterResolver](src/workflow_steps/mod.rs#L55-L199)

## Web 控制台

Web 控制台是当前的输入/输出触手：
- 输入服务：POST `/api/send/{session_id}`，并支持上传文件（`/api/upload`）
- 输出服务：GET `/api/messages/{session_id}` + SSE 订阅 `/api/subscribe`

实现位置：[web_console.rs](src/tentacles/web_console.rs)

## 运行方式

### 1) 环境变量

- `LMSTUDIO_URL`：LLM 服务地址（默认 `http://localhost:1234`）
- `LMSTUDIO_API_KEY`：可选
- `LMSTUDIO_MODEL`：模型名（默认 `default`）
- `ROBOT_MCP_SERVER_ADDR`：MCP server 地址（默认 `127.0.0.1:9001`）

### 2) 启动

在 `robot_core` 目录下运行：

```bash
cargo run
```

默认会启动：
- WebInput：0.0.0.0:8080
- WebOutput：0.0.0.0:8081

入口配置见：[main.rs](src/main.rs#L16-L83)

## 扩展点

### 1) 新增输入/输出触手（Tentacle）

- 实现 `InputHandler` / `OutputHandler`
- 用 [register_handlers!](src/macros.rs#L11-L30) 在启动时注册并配置路由

### 2) 自定义路由策略

- 通过 [EventRouter](src/core/router.rs) 将不同来源/会话的输出分发到不同 handler

### 3) 增加新的工作流 Step

扩展 [StepSpec](src/utils/mod.rs#L81-L87) 与 [build_step](src/workflow_steps/mod.rs#L273-L287)
