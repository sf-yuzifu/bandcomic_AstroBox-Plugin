# 腕上漫画同步器 (AstroBox V4 插件)

基于 Rust 和 WASI Preview 3 (Component Model) 开发的 AstroBox V4 插件，用于管理腕上漫画的漫画源、Cookie、书架数据以及本地漫画导入。

## 功能特性

- 🌐 **漫画源与 Cookie 同步**：通过 Interconnect 将漫画源配置与鉴权 Cookie 下发至穿戴设备
- 📚 **离线书架管理**：浏览手环本地已缓存漫画与源列表，支持设备端漫画/源删除
- 📤 **本地漫画导入**：
  - 支持单篇与多章节模式
  - 支持封面自定义
  - 自动根据设备参数（JPEG / PNG / LVGL indexed-8）进行图片预处理与滑动窗口分片传输
- 🚀 **内建本地 HTTP 漫画源（API Level 4）**：
   - 宿主侧监听 `0.0.0.0` 动态端口
   - 支持设备上传时优先自动绑定 `127.0.0.1:<实际端口>`，无需填写局域网 IP
   - 回环连接失败或超时后尝试已保存的备用 IPv4；绑定成功后统一使用该地址生成源配置、封面与正文 URL
  - 支持 `/control/health` 服务健康与能力检查
   - 提供 `/control/probe.jpg`、`/control/probe.png`、`/control/probe.bin` 固定探针图片，供设备端原生 fetch 导入联调

## 本地漫画导入

1. 保持 AstroBox 与设备连接，在插件「上传本地漫画」页选择图片和可选封面。
2. 点击上传。插件自动协商设备能力，支持原生 HTTP 的设备先完成健康检查和图片落盘探针，再开始下载。
3. 手环显示下载进度，保存完成后可从离线书架阅读。不支持原生 fetch 的设备或旧客户端使用互联分片导入。

通常无需配置地址。AstroBox 原生网络转发由宿主建立 TCP 连接，当前 Windows + 小米手环 9 Pro 组合已由用户确认可使用 `127.0.0.1`。每次连接仍由设备端探针验证，不将这一实测结果视为所有平台/固件均可用。

需要备用地址时，展开「连接设置」，填写运行 AstroBox 的电脑/手机的 IPv4（无需端口）。该地址保存在 `http-address.txt`；旧版本保存的地址会自动作为备用地址，回环仍优先尝试。连接失败会明确提示重试。

## 技术栈

- **语言**：Rust (Edition 2024, Rust 1.98.0)
- **架构**：WASI Preview 3 / Component Model (`psys-world-v4-http`)
- **UI 框架**：AstroBox PSYS Host V4 UI
- **通信**：Interconnect 消息通道 + 内建 HTTP 服务
- **主要依赖**：
  - `wit-bindgen` (0.62.0) - WIT 接口绑定 (async + inter-task-wakeup)
  - `waki` (0.5.1) - WASI 出站 HTTP 客户端
  - `serde_json` (1.0) - JSON 协议解析
  - `image` (0.25) - 图片缩放与编解码
  - `tracing` (0.1) - 结构化日志

## 构建与打包

### 前置要求

- Rust 工具链：1.98.0
- Target：`wasm32-wasip2`
- Python 3.x

### 构建与打包 ABP

```bash
# Windows / Linux
python scripts/build_dist.py --release --package
```

构建完成后，生成的 `.abp` 文件位于 `dist/` 目录。

## 权限声明 (manifest.json)

- `interconnect`：与手环快应用通信
- `thirdpartyapp`：检查并拉起手环快应用
- `device`：获取连接设备列表
- `register_interconnect_recv`：注册消息接收器
- `http-server`：启动本地 HTTP 服务
- `http-server.lan`：允许局域网设备访问本地 HTTP 端口

## 版本要求

- **WASI 版本**：3
- **API 级别**：4
- **AstroBox**：支持 API Level 4 的 AstroBox 版本
- **手环应用**：腕上漫画快应用 (`moe.yzf.comic`)
