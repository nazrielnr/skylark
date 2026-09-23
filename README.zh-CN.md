# Skylark

基于 [Zeron](https://github.com/zeronsh/zeron) 的本地编码 agent 桌面应用。

*[English](README.md) | 简体中文*

## 当前状态：仅本地开发

暂时停用 Skylark 账号登录/退出、云同步、组织设置、跨设备控制、本地数据导入云端和应用更新。已有云端登录凭据和账号数据保留，但不会加载。环境变量和便携包更新配置不能重新开启这些功能。

本地会话、文件、Git、终端和本地预览仍可使用。agent 提供商的登录及适配器安装不受影响；agent、Git 远程操作、依赖下载和浏览的网页仍可能访问网络。**仅本地模式不是断网沙箱。**

## 从源码运行

公共安装器、下载分发和自动发布已暂停。不要使用上游安装器安装 Skylark。安装 Rust 和对应平台的构建工具后，在当前仓库运行：

```sh
cargo run --locked -p skylark
```

只运行本地引擎：

```sh
cargo run --locked -p skylark -- headless
```

参见[运行指南](docs/RUNNING.md)、[Windows 开发说明](docs/reference/windows-development.md)和 [Linux 浏览器依赖](docs/reference/linux-browser.md)。

`skylark status` 显示本地引擎状态。`login`、`logout`、`sync` 和 `update` 返回功能已停用的错误，不访问产品服务，也不修改已有云端凭据。启动前请退出旧版 Zeron/Skylark 引擎。

## 保留的源码

- [仅本地模式说明](docs/LOCAL_ONLY.md)记录停用范围和恢复条件。
- UI 框架依赖仍来自上游仓库，版本由 Cargo 锁定。
- `edge/`、iOS 和网站源码保留，尚未作为 Skylark 服务启用。
- 云部署、应用发布及 TestFlight 工作流改为 `.yml.disabled`；源码构建和测试工作流保留。
- 历史云端文档仅供参考，不代表当前支持这些流程。
- 不会自动迁移 Zeron 数据目录。

## 许可证

基于 Zeron，遵循 [MIT License](LICENSE)。保留原始版权声明及[第三方声明](THIRD_PARTY_NOTICES.md)。更改应用名称不代表拥有上游服务。
