一个开源小软件应该是小而精美，还是应该缝合各种软件，功能越做越全呢？我的答案是缝合。
如果你的电脑上没有应用时长管理软件、日程管理软件、还有专注类的软件，可以试试我的作品。它类似于手机上应用健康使用时长+番茄todo的组合，另加了一个日程管理，用户可以用ai辅助导入导出日程，还有应用分类，这就是创新点，此外我觉得没有更多需要说明的了，软件还很不成熟，发出来完全因为前段改好了并不是全部功能做完了。如果用的上可以给我点个star，反馈可以上github issue

# 刻度 Kedu

刻度是一款 Windows 本地应用使用统计与专注工具。它记录前台应用和窗口标题，帮助你回顾时间去向，并通过专注任务和每日额度减少分心。

## 主要功能

- 统计应用使用时长，按日期查看趋势、排行和时间轴；按分类整理应用与网站。
- 通过可选的 Edge 扩展记录网站和页面使用情况，网站时间属于浏览器使用时间的一部分。
- 创建专注任务和日程，设置暂停、休息、提醒及应用白名单。
- 为应用或应用组设置每日额度，并在达到额度前提醒。
- 在洞察页面查看使用规律；提供浅色、深色外观及强调色。
- 支持本地数据备份、恢复和 CSV 导出。

## 下载安装

在 [Releases](https://github.com/chumenliuxiang/kedu/releases) 下载最新 Windows 安装包并运行。已有安装可直接更新，使用记录保存在 `%USERPROFILE%\刻度数据`。

首次使用网站统计时，在刻度的设置中打开 Edge 扩展目录，在 `edge://extensions` 开启开发人员模式，选择“加载解压缩的扩展”，然后选择该目录。扩展的记录范围与权限说明见 [Edge 扩展说明](edge-extension/README.md)。

## 数据与隐私

应用使用记录保存在本机，不需要账号。开启网站记录后，浏览器扩展通过本机回环地址将网页信息发送给刻度。窗口标题、网页标题和路径可能包含个人信息；请在分享导出的数据或备份前自行检查。可以在设置中调整记录和排除选项。

## 日程导入

在“专注 → 日程 → 导入日程”粘贴 JSON，选择日期并预览后导入。预览中可以编辑时段及关联任务。

```json
{
  "name": "学习日",
  "items": [
    { "label": "英语精读", "start": "09:00", "durationMinutes": 50, "mode": "normal" },
    { "label": "复盘整理", "start": "20:30", "durationMinutes": 30, "mode": "strict" }
  ]
}
```

同一天的时段不能重叠；`mode` 为 `normal` 或 `strict`。可以在预览中选择不关联任务或关联已有任务。

## 本地开发

需要 Windows、Node.js、pnpm、Rust 1.94.1 和 WebView2。

```powershell
pnpm install
pnpm tauri dev
```

`pnpm dev` 可在浏览器中使用示例数据预览界面。构建 Windows 安装包：

```powershell
pnpm tauri build
```

运行检查：

```powershell
pnpm build
cargo test --manifest-path src-tauri/Cargo.toml --lib
node --test tests/website_extension_unit.cjs
```

## 技术栈

Tauri 2、Rust、React、TypeScript 和 SQLite。

## 许可证

[MIT License](LICENSE)
