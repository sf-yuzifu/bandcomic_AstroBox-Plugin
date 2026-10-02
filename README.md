# 腕上漫画同步器 (AstroBox V4 插件)

基于 Rust 和 WASI Preview 3 (Component Model) 开发的 AstroBox V4 插件，用于管理腕上漫画的漫画源、Cookie、书架数据以及本地漫画导入。

## 功能特性

- 🌐 **漫画源与 Cookie 同步**：通过 Interconnect 将漫画源配置与鉴权 Cookie 下发至穿戴设备
- 📚 **离线书架管理**：浏览手环本地已缓存漫画与源列表，支持设备端漫画/源删除
- 📤 **本地漫画导入**：
  - 支持单篇与多章节模式
  - 支持封面自定义
  - 自动根据设备参数（JPEG / PNG / LVGL indexed-8）进行图片预处理与滑动窗口分片传输
- 🚀 **内建 HTTP 探针服务（API Level 4）**：
  - 宿主侧监听 `0.0.0.0` 动态端口
  - 支持 `/control/health` 服务健康与能力检查
  - 提供 `/control/probe.jpg`、`/control/probe.png`、`/control/probe.bin` 固定探针图片，供设备端原生 fetch 导入联调

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
