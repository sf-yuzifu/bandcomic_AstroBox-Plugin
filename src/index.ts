import AstroBox, { PluginUINode } from "astrobox-plugin-sdk";

// --- 1. 定义常量和类型 ---

const WATCH_APP_PKG_NAME = "moe.yzf.comic";
const CONFIG_KEY_COOKIE = "savedCookie";
const CONFIG_KEY_DOMAIN = "sourceDomain";
const CONFIG_KEY_SOURCE_NAME = "sourceName";

interface PluginConfig {
  [CONFIG_KEY_COOKIE]?: string;
  [CONFIG_KEY_DOMAIN]?: string;
  [CONFIG_KEY_SOURCE_NAME]?: string;
}

let currentCookieInput: string = "";
let currentDomainInput: string = "";
let fetchedSourceName: string | null = null;
let ui: PluginUINode[] = [];

const sleep = (delay: number) => new Promise((resolve) => setTimeout(resolve, delay));

// --- 2. 业务逻辑函数 ---

async function fetchSourceName(domain: string): Promise<string | null> {
  try {
    const normalizedDomain = domain.replace(/\/$/, "");
    const configUrl = `${normalizedDomain}/config`;

    console.log(`请求配置 URL: ${configUrl}`);
    
    const res = await AstroBox.network.fetch(configUrl,{
      method: "GET",
      headers: {
        "Content-Type": "application/json",
      },
      raw: false,
    });

    if (res.status !== 200) {
      console.error(`获取配置失败，状态码: ${res.status}`);
      return null;
    }

    const configData = JSON.parse(res.body);
    const sourceNames = Object.keys(configData);

    console.log(`配置中的 sourceNames: ${sourceNames}`);
    
    if (sourceNames.length === 0) {
      console.error("配置中没有找到 sourceName");
      return null;
    }

    const sourceName = sourceNames[0];
    console.log(`成功获取 sourceName: ${sourceName}`);
    return sourceName;
  } catch (error) {
    console.error("获取漫画源配置失败:", error);
    return null;
  }
}

async function showStatusMessage(
  status: "default" | "processing" | "success" | "error",
  message?: string
) {
  const statusNodeId = "status_message";
  
  let htmlValue = "";
  switch (status) {
    case "processing":
      htmlValue = `<span style="display:inline-block;width:100%;text-align:center;color:#1890ff;">${message || "处理中..."}</span>`;
      break;
    case "success":
      htmlValue = `<span style="display:inline-block;width:100%;text-align:center;font-weight:bold;color:#52c41a;">${message || "同步成功！"}</span>`;
      break;
    case "error":
      htmlValue = `<span style="display:inline-block;width:100%;text-align:center;font-weight:bold;color:#ff4d4f;">错误：${message || "未知错误"}</span>`;
      break;
    default:
      htmlValue = currentCookieInput
        ? `<span style="display:inline-block;width:100%;text-align:center;color:#666;">已加载上次保存的 Cookie，可直接同步。</span>`
        : `<span style="display:inline-block;width:100%;text-align:center;color:#666;">请输入漫画源域名和 Cookie。</span>`;
  }

  ui[6] = {
    node_id: statusNodeId,
    visibility: true,
    disabled: false,
    content: {
      type: "HtmlDocument",
      value: htmlValue,
    },
  };
  
  AstroBox.ui.updatePluginSettingsUI(ui);

  if (status === "success" || status === "error") {
    await sleep(3000);
    ui[6].visibility = false;
    AstroBox.ui.updatePluginSettingsUI(ui);
  }
}

function updateUI() {
  ui = [
    {
      node_id: "domain_label",
      visibility: true,
      disabled: false,
      content: {
        type: "HtmlDocument",
        value: `
          <span style="margin-left: 13%;">漫画源域名<span style="color:#666;font-size:12px;">（例如：https://youapi.domain）</span></span>
        `,
      },
    },
    {
      node_id: "domain_input",
      visibility: true,
      disabled: false,
      content: {
        type: "Input",
        value: { text: currentDomainInput, callback_fun_id: domainChangeFunId },
      },
    },
    {
      node_id: "source_name_label",
      visibility: true,
      disabled: false,
      content: {
        type: "HtmlDocument",
        value: `
          <span style="margin-left: 13%;">漫画源名称<span style="color:#666;font-size:12px;">（自动获取）</span></span>
        `,
      },
    },
    {
      node_id: "source_name_input",
      visibility: true,
      disabled: true,
      content: {
        type: "Input",
        value: { text: fetchedSourceName || "", callback_fun_id: "" },
      },
    },
    {
      node_id: "cookie_label",
      visibility: true,
      disabled: false,
      content: {
        type: "HtmlDocument",
        value: `
          <span style="margin-left: 13%;">Cookie<span style="color:#666;font-size:12px;">（从浏览器开发者工具获取）</span></span>
        `,
      },
    },
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
      node_id: "status_message",
      visibility: false,
      disabled: false,
      content: {
        type: "HtmlDocument",
        value: "",
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

  AstroBox.ui.updatePluginSettingsUI(ui);
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

async function onDomainInputChange(inputValue: string) {
  console.log("漫画源域名变化:", inputValue);
  currentDomainInput = inputValue;
  fetchedSourceName = null;

  try {
    const cfg = AstroBox.config.readConfig() as PluginConfig;
    AstroBox.config.writeConfig({
      ...cfg,
      [CONFIG_KEY_DOMAIN]: inputValue,
    });
    console.log("漫画源域名已实时保存到配置。");
  } catch (error) {
    console.error("实时保存漫画源域名到配置失败:", error);
  }

  if (inputValue && inputValue.includes(".")) {
    await showStatusMessage("processing", "正在获取漫画源配置...");
    const sourceName = await fetchSourceName(inputValue);
    if (sourceName) {
      fetchedSourceName = sourceName;
      try {
        const cfg = AstroBox.config.readConfig() as PluginConfig;
        AstroBox.config.writeConfig({
          ...cfg,
          [CONFIG_KEY_SOURCE_NAME]: sourceName,
        });
        console.log("漫画源名称已保存到配置。");
      } catch (error) {
        console.error("保存漫画源名称到配置失败:", error);
      }
      ui[3].visibility = false;
      AstroBox.ui.updatePluginSettingsUI(ui);
      await sleep(100);
      ui[3] = {
        node_id: "source_name_input",
        visibility: true,
        disabled: true,
        content: {
          type: "Input",
          value: { text: fetchedSourceName, callback_fun_id: "" },
        },
      };
      AstroBox.ui.updatePluginSettingsUI(ui);
      await showStatusMessage("success", `获取成功：${sourceName}`);
    } else {
      fetchedSourceName = null;
      try {
        const cfg = AstroBox.config.readConfig() as PluginConfig;
        AstroBox.config.writeConfig({
          ...cfg,
          [CONFIG_KEY_SOURCE_NAME]: "",
        });
      } catch (error) {
        console.error("清空漫画源名称配置失败:", error);
      }
      ui[3].visibility = false;
      AstroBox.ui.updatePluginSettingsUI(ui);
      await sleep(100);
      ui[3] = {
        node_id: "source_name_input",
        visibility: true,
        disabled: true,
        content: {
          type: "Input",
          value: { text: "", callback_fun_id: "" },
        },
      };
      AstroBox.ui.updatePluginSettingsUI(ui);
      await showStatusMessage("error", "无法获取漫画源配置，请检查域名是否正确。");
    }
  }
}

async function handleSync() {
  const cookieInput = currentCookieInput;
  const domainInput = currentDomainInput;

  await showStatusMessage("processing", "正在验证输入...");

  if (!cookieInput) {
    await showStatusMessage("error", "Cookie 不能为空。");
    return;
  }

  if (!domainInput) {
    await showStatusMessage("error", "漫画源域名不能为空。");
    return;
  }

  await showStatusMessage("processing", "正在获取漫画源配置...");

  let sourceName = fetchedSourceName;
  if (!sourceName) {
    sourceName = await fetchSourceName(domainInput);
    if (!sourceName) {
      await showStatusMessage("error", "无法获取漫画源配置，请检查域名是否正确。");
      return;
    }
    fetchedSourceName = sourceName;
  }

  await showStatusMessage("processing", "正在检查快应用...");

  try {
    const appList = await AstroBox.thirdpartyapp.getThirdPartyAppList();
    const app = appList.find((app) => app.package_name == WATCH_APP_PKG_NAME);
    
    if (!app) {
      await showStatusMessage("error", "请先安装腕上漫画快应用！");
      return;
    }
    
    if (app.version_code < 153) {
      await showStatusMessage("error", "请先安装腕上漫画快应用的新版本！");
      return;
    }

    await AstroBox.thirdpartyapp.launchQA(app, "/pages/index");
    await sleep(2000);

    await showStatusMessage("processing", "正在发送到手表...");

    const cookieData = JSON.stringify({
      [sourceName]: cookieInput
    });
    
    await AstroBox.interconnect.sendQAICMessage(
      WATCH_APP_PKG_NAME,
      cookieData
    );
    await showStatusMessage("success", "同步成功！");
  } catch (error) {
    console.error(error);
    await showStatusMessage("error", "发送失败，请检查手表连接和应用是否打开。");
  }
}

// --- 3. 注册原生函数 ---

const syncFunId = AstroBox.native.regNativeFun(handleSync);
const inputChangeFunId = AstroBox.native.regNativeFun(onCookieInputChange);
const domainChangeFunId = AstroBox.native.regNativeFun(onDomainInputChange);

// --- 4. 插件生命周期 ---

AstroBox.lifecycle.onLoad(() => {
  console.log("bandcomic Helper 插件已加载...");

  try {
    const cfg = AstroBox.config.readConfig() as PluginConfig;
    if (cfg && cfg[CONFIG_KEY_COOKIE]) {
      currentCookieInput = cfg[CONFIG_KEY_COOKIE]!;
      console.log("成功从配置中加载已保存的 Cookie。");
    }
    if (cfg && cfg[CONFIG_KEY_DOMAIN]) {
      currentDomainInput = cfg[CONFIG_KEY_DOMAIN]!;
      console.log("成功从配置中加载已保存的漫画源域名。");
    }
    if (cfg && cfg[CONFIG_KEY_SOURCE_NAME]) {
      fetchedSourceName = cfg[CONFIG_KEY_SOURCE_NAME]!;
      console.log("成功从配置中加载已保存的漫画源名称。");
    }
  } catch (error) {
    console.error("读取插件配置失败:", error);
  }

  updateUI();
  console.log("UI 已初始渲染。");
});
