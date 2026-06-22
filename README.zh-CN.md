<p align="center">
  <img src="assets/icons/codex-migrate-512.png" width="112" alt="Codex Migrate 图标">
</p>

<h1 align="center">Codex Migrate</h1>

<p align="center">
  本地优先的跨平台 Codex 会话迁移、路径修复、备份与 HTML 导出工具。
</p>

<p align="center">
  <a href="README.md">English</a> ·
  <a href="#下载安装">下载安装</a> ·
  <a href="#安全与回滚">安全与回滚</a> ·
  <a href="CONTRIBUTING.md">参与贡献</a>
</p>

> [!IMPORTANT]
> Codex Migrate 是独立的社区项目，与 OpenAI 不存在隶属、赞助或官方背书关系。“Codex”及其他 OpenAI 标识归 OpenAI 所有。

Codex Migrate 可以直接读取旧设备复制出来的 `.codex/`，也兼容旧版生成的
精简 `Codex/` 文件夹。新版备份会完整复制 `.codex/`。导入时程序以 rollout
JSONL 为会话事实来源，将用户选中的会话安全合并到已有的本机 Codex 环境。

## 软件截图

<p align="center">
  <img src="docs/screenshots/app-overview.png" width="920" alt="Codex Migrate 软件界面总览">
</p>

<table>
  <tr>
    <td width="68%">
      <img src="docs/screenshots/migration-workflow.png" alt="三步会话迁移流程">
    </td>
    <td width="32%">
      <img src="docs/screenshots/feature-navigation.png" alt="迁移、备份、路径修复、HTML 导出、回滚和设置导航">
    </td>
  </tr>
  <tr>
    <td align="center">三步迁移流程</td>
    <td align="center">迁移与维护功能入口</td>
  </tr>
</table>

## 主要功能

- 支持 macOS、Windows、Linux 和 WSL 之间迁移活动及归档会话。
- 按项目和会话单独选择要导入的内容。
- 将旧设备项目路径映射到新设备的真实文件夹。
- 通过父目录映射批量匹配多个项目。
- 修复本机现有 `.codex` 会话中的项目路径。
- 检测重复、完整前缀版本及同 UUID 分叉冲突。
- 写入前创建回滚快照，并可在 GUI 中批量删除旧快照。
- 将会话导出为单文件 HTML，内嵌用户图片和工具截图。
- 中英文原生 GUI，默认跟随系统语言。
- GUI 和 CLI 共用同一套迁移核心。

## 备份内容

备份功能会将所选 Codex 主目录下的全部文件和文件夹复制到：

```text
所选目录/
└── .codex/
```

其中包括活动及归档会话、SQLite 数据库、Skills、配置、插件、日志、缓存、回滚记录，
以及来源目录中的其他内容。根目录下的 `auth.json` 等登录凭据文件会被明确排除。

> [!WARNING]
> 即使不包含登录凭据，完整备份仍会包含私密对话、命令输出、图片、本机路径、
> 配置和日志。请妥善保管，不要公开分享。导出前应完全关闭 Codex，确保数据库及
> WAL 文件复制一致。

## 下载安装

在 GitHub 仓库的 **Releases** 页面下载对应平台版本：

- Windows：解压 ZIP，运行 `Codex Migrate.exe`。
- macOS：解压 ZIP，将 `Codex Migrate.app` 移入“应用程序”。首次启动时按住
  Control 点击应用并选择“打开”；如果仍被阻止，请前往“系统设置 → 隐私与安全性”
  点击“仍要打开”。
- Linux：解压压缩包，运行 `codex-migrate-gui`。

当前发布包没有使用商业代码签名证书，也未经过 Apple 公证。Windows SmartScreen
或 macOS Gatekeeper 可能显示提示。运行下载程序前请核对 Release 中的 SHA-256
校验文件。

## 从源码构建

需要 Rust stable 工具链，以及 `eframe` 所需的平台构建环境。

```bash
git clone https://github.com/ChenglongLi777/codex-migrate.git
cd codex-migrate
cargo test --all-targets --features gui
cargo build --release --features gui --bins
```

macOS 应用打包：

```bash
./scripts/package-macos.sh
```

## GUI 使用流程

1. 完全退出 Codex Desktop 和所有 Codex CLI 会话。
2. 选择旧设备 `.codex/`、旧版精简 `Codex/`，或只包含其中一个目录的父目录。
3. 选择需要导入的项目与会话。
4. 为每个已选项目绑定新设备文件夹、应用父目录映射，或选择“仅恢复历史”。
5. 预览新增、重复、较长版本及冲突。
6. 确认导入，完成后重新打开 Codex。

## 合并规则

| 来源与目标状态 | 处理方式 |
| --- | --- |
| 目标不存在 UUID | 导入 |
| UUID 和内容哈希一致 | 跳过并刷新索引 |
| 目标 rollout 是来源的完整前缀 | 使用较长来源 |
| 来源 rollout 是目标的完整前缀 | 保留较长目标 |
| 同 UUID 内容分叉 | 停止并报告冲突 |

程序不会拼接分叉 JSONL，也不会静默生成新 UUID。

## 安全与回滚

- 检查目标 SQLite 是否正被占用。
- 使用 SQLite Online Backup 创建数据库快照。
- rollout 文件先进入 staging，再移动到目标目录。
- 只更新运行时检测到的数据库字段。
- 导入失败时自动恢复快照。
- 回滚数据保存在：

```text
$CODEX_HOME/migration_transactions/<事务ID>/
```

删除回滚数据只会删除所选备份目录，不会修改当前会话。

## 常用 CLI 命令

```bash
codex-migrate export ~/.codex --output-parent ~/Backups
codex-migrate scan ~/Backups/.codex

codex-migrate import ~/Backups/.codex --dry-run \
  --map '/Users/alex/Projects=D:/Projects'

codex-migrate rebind --codex-home ~/.codex \
  --map '/旧项目父目录=/新项目父目录' --dry-run

codex-migrate export-html --codex-home ~/.codex --thread THREAD_ID
codex-migrate verify
codex-migrate rollback TRANSACTION_ID
```

完整参数请运行 `codex-migrate --help`。

## 项目状态

本工具依赖 Codex 本地存储结构，而这些结构可能随 Codex 版本变化。欢迎提交兼容性问题和修复。提交 Issue 前，请从日志和截图中删除对话内容、凭据、用户名和私有路径。

## 许可证

MIT，详见 [LICENSE](LICENSE)。
