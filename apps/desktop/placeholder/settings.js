const invoke = window.__TAURI_INTERNALS__.invoke;

const form = document.querySelector("#settings-form");
const serverUrl = document.querySelector("#server-url");
const apiKey = document.querySelector("#api-key");
const apiKeyHint = document.querySelector("#api-key-hint");
const toggleKey = document.querySelector("#toggle-key");
const iconShow = document.querySelector("#icon-show");
const iconHide = document.querySelector("#icon-hide");
const saveButton = document.querySelector("#save-button");
const saveResult = document.querySelector("#save-result");
const credentialWarning = document.querySelector("#credential-warning");
const connectionBadge = document.querySelector("#connection-badge");
const lastPull = document.querySelector("#last-pull");
const snapshotCount = document.querySelector("#snapshot-count");
const lastError = document.querySelector("#last-error");

const SUBMIT_LABEL = "测试连接并保存";

function formatTime(value) {
  if (!value) return null;
  const date = new Date(value);
  return Number.isNaN(date.valueOf()) ? value : date.toLocaleString("zh-CN");
}

/** 文本与「是否空值」的样式一起设置，避免上一次的状态色残留到下一次渲染。 */
function setValue(node, text, kind) {
  node.textContent = text;
  node.classList.toggle("is-empty", kind === "empty");
  node.classList.toggle("is-error", kind === "error");
}

function setBadge(state) {
  const connected = Boolean(state.serverUrl && state.hasApiKey);
  const failing = Boolean(state.lastError);
  connectionBadge.classList.remove("ok", "pending", "fail");
  if (!connected) {
    connectionBadge.classList.add("pending");
    connectionBadge.textContent = "未连接";
  } else if (failing) {
    connectionBadge.classList.add("fail");
    connectionBadge.textContent = "拉取异常";
  } else {
    connectionBadge.classList.add("ok");
    connectionBadge.textContent = "已连接";
  }
}

function render(state) {
  if (state.serverUrl && !serverUrl.value) serverUrl.value = state.serverUrl;

  apiKeyHint.textContent = state.hasApiKey
    ? "已保存密钥，留空即继续使用。保存前会实际连接服务器验证。"
    : "在服务器上执行 zhiyu-api-key 签发，明文只显示一次。";

  credentialWarning.hidden = !state.credentialWarning;
  credentialWarning.textContent = state.credentialWarning || "";

  setBadge(state);

  const pulled = formatTime(state.lastPullAt);
  setValue(lastPull, pulled || "尚未拉取", pulled ? null : "empty");
  setValue(snapshotCount, String(state.localSnapshotCount), null);
  setValue(lastError, state.lastError || "无", state.lastError ? "error" : "empty");
}

function showResult(message, kind) {
  saveResult.hidden = false;
  saveResult.classList.remove("success", "error");
  saveResult.classList.add(kind);
  saveResult.textContent = message;
}

async function refresh() {
  try {
    render(await invoke("get_backup_settings"));
  } catch (error) {
    showResult(`读不到当前设置：${String(error)}`, "error");
  }
}

toggleKey.addEventListener("click", () => {
  const revealed = apiKey.type === "text";
  apiKey.type = revealed ? "password" : "text";
  iconShow.hidden = !revealed;
  iconHide.hidden = revealed;
  toggleKey.setAttribute("aria-label", revealed ? "显示 api-key" : "隐藏 api-key");
  apiKey.focus();
});

form.addEventListener("submit", async (event) => {
  event.preventDefault();

  // 客户端只挡住肯定不成立的情况，真正的判定交给服务器——本地看着合法的地址
  // 也可能不可达、证书无效或密钥已撤销。
  if (!serverUrl.value.trim()) {
    serverUrl.setAttribute("aria-invalid", "true");
    serverUrl.focus();
    showResult("请先填写服务器地址。", "error");
    return;
  }
  serverUrl.removeAttribute("aria-invalid");

  saveButton.disabled = true;
  saveButton.textContent = "正在连接…";
  saveResult.hidden = true;

  try {
    const state = await invoke("save_backup_settings", {
      input: { serverUrl: serverUrl.value.trim(), apiKey: apiKey.value },
    });
    apiKey.value = "";
    render(state);
    showResult("已保存。正在进入知余，备份拉取已在后台开始。", "success");
  } catch (error) {
    // 失败原因来自 Rust 侧，已是可读中文（地址不可达 / 密钥无效 / 证书问题…），
    // 直接透传，不要用「保存失败」把它盖掉。
    showResult(String(error), "error");
    saveButton.disabled = false;
    saveButton.textContent = SUBMIT_LABEL;
    return;
  }

  // 成功后按钮保持禁用与「已保存」，窗口即将关闭，不要闪回初始文案。
  saveButton.textContent = "已保存";
});

refresh();
setInterval(refresh, 30_000);
