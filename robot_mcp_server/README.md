# robot_mcp_server（简要说明）

MCP 服务器示例，提供多种工具（天气、当前时间、Echo、求和、FFprobe、Profile、Chat 等），可通过配置扩展外部工具。

## 位置
- 入口代码： [src/main.rs](src/main.rs)
- 工程配置： [Cargo.toml](Cargo.toml)
- 外部工具配置： [config/external.toml](config/external.toml)

## 运行
```bash
cargo run
```

## 提示
- 工具实现位于 [src/tools/](src/tools/) 目录。  
- 如需新增工具，参考现有文件结构与注册方式。

