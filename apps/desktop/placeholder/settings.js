const invoke = window.__TAURI_INTERNALS__.invoke;
const form = document.querySelector("#settings-form");
const serverUrl = document.querySelector("#server-url");
const apiKey = document.querySelector("#api-key");
const apiKeyHint = document.querySelector("#api-key-hint");
const saveButton = document.querySelector("#save-button");
const saveResult = document.querySelector("#save-result");
const credentialWarning = document.querySelector("#credential-warning");
const lastPull = document.querySelector("#last-pull");
const snapshotCount = document.querySelector("#snapshot-count");
const lastError = document.querySelector("#last-error");

function formatTime(value) {
  if (!value) return "尚未成功拉取";
  const date = new Date(value);
  return Number.isNaN(date.valueOf()) ? value : date.toLocaleString("zh-CN");
}

function render(state) {
  if (state.serverUrl && !serverUrl.value) serverUrl.value = state.serverUrl;
  apiKeyHint.textContent = state.hasApiKey
    ? "凭证已保存；留空可继续使用现有 api-key。保存前会实际连接服务器验证。"
    : "首次设置必须填写。保存前会实际连接服务器验证。";
  credentialWarning.hidden = !state.credentialWarning;
  credentialWarning.textContent = state.credentialWarning || "";
  lastPull.textContent = formatTime(state.lastPullAt);
  snapshotCount.textContent = String(state.localSnapshotCount);
  lastError.textContent = state.lastError || "无";
}

function showResult(message, isError) {
  saveResult.hidden = false;
  saveResult.classList.toggle("error", isError);
  saveResult.textContent = message;
}

async function refresh() {
  try {
    render(await invoke("get_backup_settings"));
  } catch (error) {
    showResult(`读取设置失败：${String(error)}`, true);
  }
}

form.addEventListener("submit", async (event) => {
  event.preventDefault();
  saveButton.disabled = true;
  saveButton.textContent = "正在连接…";
  saveResult.hidden = true;
  try {
    const state = await invoke("save_backup_settings", {
      input: { serverUrl: serverUrl.value, apiKey: apiKey.value },
    });
    apiKey.value = "";
    render(state);
    showResult("连接验证成功，设置已保存。正在进入知余，备份拉取也已在后台启动。", false);
  } catch (error) {
    showResult(`保存失败：${String(error)}`, true);
  } finally {
    saveButton.disabled = false;
    saveButton.textContent = "测试连接并保存";
  }
});

refresh();
setInterval(refresh, 30_000);
