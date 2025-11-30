import AstroBox, { PluginUINode } from "astrobox-plugin-sdk";

// --- 1. 定义常量和类型 ---

const WATCH_APP_PKG_NAME = "moe.yzf.comic"; // **注意**: 包名已根据您的反馈修正
const CONFIG_KEY_COOKIE = "savedCookie";

interface PluginConfig {
  [CONFIG_KEY_COOKIE]?: string;
}

let currentCookieInput: string = "";

// --- 2. 业务逻辑函数 ---

/**
 * **核心修改**: 智能提取 MUSIC_U
 * 既能处理完整 cookie 字符串，也能处理用户直接粘贴的 MUSIC_U 值
 */
function extractCookie(fullCookie: string): string | null {
  if (!fullCookie || typeof fullCookie !== "string") {
    return null;
  }

  return fullCookie; // 如果两种情况都不匹配，则返回 null
}

/**
 * **核心修改**: 更新状态时，必须包含所有需要保持可见的节点
 */
function updateStatus(
  status: "default" | "processing" | "success" | "error",
  message?: string
) {
  // 准备所有需要保持可见的节点
  const commonNodes: PluginUINode[] = [
    {
      node_id: "cookie_input",
      visibility: true,
      disabled: false,
      content: {
        type: "Input",
        value: { text: currentCookieInput, callback_fun_id: inputChangeFunId },
      },
    },
    {
      node_id: "sync_button",
      visibility: true,
      disabled: false,
      content: {
        type: "Button",
        value: {
          primary: true,
          text: "同步到手表",
          callback_fun_id: syncFunId,
        },
      },
    },
  ];

  // 准备所有状态文本节点，并根据当前状态设置其可见性
  const statusNodes: PluginUINode[] = [
    {
      node_id: "status_text_default",
      visibility: status === "default",
      disabled: false,
      content: {
        type: "Text",
        value: currentCookieInput
          ? "已加载上次保存的 Cookie，可直接同步。"
          : "请从浏览器开发者工具获取 Cookie 并粘贴。",
      },
    },
    {
      node_id: "status_text_processing",
      visibility: status === "processing",
      disabled: false,
      content: { type: "Text", value: message || "处理中..." },
    },
    {
      node_id: "status_text_success",
      visibility: status === "success",
      disabled: false,
      content: { type: "Text", value: "同步成功！" },
    },
    {
      node_id: "status_text_error",
      visibility: status === "error",
      disabled: false,
      content: { type: "Text", value: `错误：${message || "未知错误"}` },
    },
  ];

  // 将通用节点和状态节点合并，一次性更新所有UI
  AstroBox.ui.updatePluginSettingsUI([...commonNodes, ...statusNodes]);
}

function onCookieInputChange(inputValue: string) {
  console.log("输入框内容变化:", inputValue);
  currentCookieInput = inputValue;

  try {
    const cfg = AstroBox.config.readConfig() as PluginConfig;
    AstroBox.config.writeConfig({
      ...cfg,
      [CONFIG_KEY_COOKIE]: inputValue,
    });
    console.log("Cookie 已实时保存到配置。");
  } catch (error) {
    console.error("实时保存 Cookie 到配置失败:", error);
  }
}

async function handleSync() {
  const cookieInput = currentCookieInput;

  updateStatus("processing", "正在提取凭证...");

  if (!cookieInput) {
    updateStatus("error", "输入框内容为空。");
    return;
  }

  const Cookie = extractCookie(cookieInput);

  if (!Cookie) {
    updateStatus("error", "凭证格式无效，请检查输入。");
    return;
  }

  updateStatus("processing", "凭证提取成功，正在发送到手表...");

  try {
    await AstroBox.interconnect.sendQAICMessage(
      WATCH_APP_PKG_NAME,
      Cookie
    );
    updateStatus("success");

    setTimeout(() => {
      updateStatus("default");
    }, 3000);
  } catch (error) {
    updateStatus("error", "发送失败，请检查手表连接和应用是否打开。");
  }
}

// --- 3. 注册原生函数 ---

const syncFunId = AstroBox.native.regNativeFun(handleSync);
const inputChangeFunId = AstroBox.native.regNativeFun(onCookieInputChange);

// --- 4. 插件生命周期 ---

AstroBox.lifecycle.onLoad(() => {
  console.log("bandcomic Helper 插件已加载...");

  try {
    const cfg = AstroBox.config.readConfig() as PluginConfig;
    if (cfg && cfg[CONFIG_KEY_COOKIE]) {
      currentCookieInput = cfg[CONFIG_KEY_COOKIE]!;
      console.log("成功从配置中加载已保存的 Cookie。");
    }
  } catch (error) {
    console.error("读取插件配置失败:", error);
  }

  // 初始渲染时，直接调用 updateStatus 来构建完整的 UI
  updateStatus("default");
  console.log("UI 已初始渲染。");
});
