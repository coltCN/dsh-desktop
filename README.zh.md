# dsh desktop

[English](README.md)

**dsh desktop** 是一个开源的 [Tauri 2](https://v2.tauri.app) 桌面客户端，服务于
[DeepSeek Harness](https://github.com/deepseek-ai/deepseek-harness)（`dsh`）。

三条设计原则：

1. **仓库内不包含 dsh 代码** —— 不 vendoring、不 fork、不 patch harness。
2. **按需用官方命令安装** —— 启动时探测系统里是否有 `dsh`；没有则执行官方安装
   （`npm install -g @deepseek-ai/dsh`），并用官方 `dsh plugin` 命令组装一个专用
   **`desktop` profile**（`$DSH_HOME/profiles/desktop`）。
3. **客户端 ↔ dsh 的通讯以 dsh 插件方式实现** —— 壳进程对 harness 的一切调用都经由
   本仓库自研并装入 desktop profile 的 **dsh-desktop-bridge** 插件，绝不抓日志、
   不碰 harness 内部。

## 工作原理

```
┌─────────────────────────── dsh desktop（本仓库）───────────────────────────┐
│  Tauri 2 壳（Rust）                                                         │
│    1. 探测 dsh → 2. 缺失则 npm 安装 → 3. 确保 desktop profile（dsh plugin）  │
│    4. spawn `dsh --profile desktop --no-open`                               │
│    5. 读 stdout 标记行，导航到带 token 的认证 URL                            │
│         │ stdout: `dsh-desktop-ready …` / `dsh web: …?token=`               │
│         ▼                                                                   │
│  dsh（npm 安装，位于本仓库之外）                                             │
│    profile desktop = bundles（按序）：                                       │
│      @deepseek-ai/dsh-base       官方核心                                    │
│      @deepseek-ai/dsh-web-app    官方 Web UI + webserver                     │
│      dsh-desktop-bridge          本仓库插件（packages/bridge）               │
│    插件打印机器可读就绪行，并在 127.0.0.1 提供                              │
│    GET /api/desktop/health · /api/desktop/info                               │
└──────────────────────────────────────────────────────────────────────────────┘
```

**界面就是 harness 官方的 Web UI**（壳在 harness 宣告的认证 URL 处打开它）；
**壳进程发出的控制面流量**（定位后端、健康检查、info、未来的 RPC）走插件注册的 API。
关窗时对后端发优雅 SIGTERM（5 秒 drain，超时再 SIGKILL）。

## 目录

```
packages/bridge/   dsh-desktop-bridge —— dsh 插件（bundle），打开控制通道。
                   零运行时依赖，不 import harness 任何代码。
apps/desktop/      Tauri 2 客户端：启动页 UI（ui/）、Rust 壳（src-tauri/）。
```

## 依赖

- **Node.js ≥ 22.19** 与 **pnpm**（dsh 与 profile 工具链需要；客户端自己装 dsh）
- 构建桌面应用需要 **Rust** 工具链

## 开发

```sh
pnpm install          # 工作区依赖（@tauri-apps/cli、bridge 构建链）
pnpm build:bridge     # 编译 packages/bridge → lib/

pnpm desktop          # tauri dev（调试构建）
pnpm desktop:build    # tauri build → 发布包
```

壳进程环境变量：

| 变量 | 含义 |
|---|---|
| `DSH_DESKTOP_DSH` | 指定 dsh 可执行文件，替代 PATH 探测 |
| `DSH_DESKTOP_BRIDGE_PACKAGE` | bridge 的安装 spec（默认发布名 `dsh-desktop-bridge`；首版发布前可指向本地打包的 tarball） |
| `DSH_HOME` | profile 位置（默认 `~/.dsh`） |

日志在 `~/.dsh-desktop/`（`backend.log`、`install.log`、`profile-add.log`、`dsh-desktop.log`）。

## desktop profile 与插件（亦可纯命令行复现）

```sh
npm install -g @deepseek-ai/dsh
dsh plugin --profile desktop add @deepseek-ai/dsh-web-app
dsh plugin --profile desktop add dsh-desktop-bridge
dsh --profile desktop --dump-config   # 只看组合结果不启动
dsh --profile desktop --no-open       # 与客户端相同的启动方式
```

插件与控制 API 契约见 [`packages/bridge/README.md`](packages/bridge/README.md)；
dsh 插件开发遵循官方
[publish 教程](https://deepseek-harness.github.io/deepseek-harness/user/develop/basic/publish.zh.html)。

## 状态

可用且克制的 v0.1：按需安装、profile 组装、启动、插件控制通道均已端到端验证。
发布打包（dmg/nsis）与代码签名尚未配置。

## License

MIT。本项目是独立的社区项目，与 DeepSeek 无隶属关系；`dsh` / DeepSeek Harness
归 DeepSeek AI 所有，从官方 npm 渠道安装，绝不捆绑在本仓库中。
