pub const HTML: &str = r#"<!doctype html>
<html lang="zh-CN">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width,initial-scale=1">
  <title>知余 · 备份设置</title>
  <style>
    :root{color-scheme:light;--bg:#fbfaf8;--card:#fff;--text:#2f2f2f;--muted:#77716a;--line:#e8e3dc;--input:#d8d3cc;--primary:#b6533c;--primary-hover:#9f4532;--soft:#f7ece9;--success:#3e8c7d;--warning:#be7c32;--danger:#b6533c;--shadow:0 10px 30px rgba(58,45,35,.06)}
    *{box-sizing:border-box}body{margin:0;background:var(--bg);color:var(--text);font:14px/1.55 -apple-system,BlinkMacSystemFont,"Segoe UI",sans-serif}main{width:min(680px,calc(100% - 36px));margin:0 auto;padding:38px 0 48px}header{margin-bottom:24px}h1{margin:0 0 5px;font-size:24px;letter-spacing:-.02em}h2{margin:0;font-size:16px}p{margin:0}.subtitle,.help{color:var(--muted)}.card{margin-top:14px;padding:20px;background:var(--card);border:1px solid var(--line);border-radius:14px;box-shadow:var(--shadow)}.card-head{display:flex;align-items:center;justify-content:space-between;gap:12px;margin-bottom:17px}.field{margin-top:14px}.field:first-child{margin-top:0}label{display:block;margin-bottom:6px;font-size:13px;font-weight:600}input[type=text]{width:100%;height:39px;padding:0 11px;color:var(--text);background:var(--card);border:1px solid var(--input);border-radius:8px;outline:none;font:inherit}input[type=text]:focus{border-color:var(--primary);box-shadow:0 0 0 3px rgba(182,83,60,.13)}.field-row{display:grid;grid-template-columns:1fr 1fr;gap:12px}.validation{margin-top:14px;padding:11px 12px;border-radius:8px;background:var(--bg);color:var(--muted)}.validation.good{background:rgba(62,140,125,.1);color:var(--success)}.validation.bad{background:rgba(182,83,60,.09);color:var(--danger)}.checks{display:flex;flex-wrap:wrap;gap:7px;margin-top:8px}.check{padding:2px 7px;border:1px solid var(--line);border-radius:999px;font-size:12px}.check.ok{border-color:rgba(62,140,125,.35);color:var(--success)}button{height:38px;padding:0 15px;border:0;border-radius:8px;background:var(--primary);color:#fff;font:600 14px/1 inherit;cursor:pointer}button:hover{background:var(--primary-hover)}button:focus-visible{outline:3px solid rgba(182,83,60,.22);outline-offset:2px}button:disabled{opacity:.55;cursor:default}.save-row{display:flex;align-items:center;gap:12px;margin-top:17px}.feedback{color:var(--muted);font-size:13px}.states{display:grid;grid-template-columns:repeat(3,1fr);gap:9px}.state{min-width:0;padding:13px;background:var(--bg);border-radius:9px}.state strong{display:block;margin-bottom:5px;font-size:12px}.state time{display:block;overflow:hidden;color:var(--muted);font-size:12px;text-overflow:ellipsis}.state.remote{background:rgba(62,140,125,.09)}.state.remote strong{color:var(--success)}.status-meta{display:grid;grid-template-columns:1fr 1fr;gap:10px;margin-top:12px}.meta{padding:11px 13px;border:1px solid var(--line);border-radius:9px}.meta span{display:block;color:var(--muted);font-size:12px}.meta strong{display:block;margin-top:3px}.warning,.error{display:none;margin-top:12px;padding:11px 12px;border-radius:8px}.warning{background:rgba(190,124,50,.11);color:var(--warning)}.error{white-space:pre-wrap;background:rgba(182,83,60,.09);color:var(--danger)}.actions{display:flex;align-items:center;gap:12px;margin-top:15px}.running{color:var(--muted)}.switch-row{display:flex;align-items:flex-start;justify-content:space-between;gap:18px}.switch{position:relative;flex:none;width:43px;height:25px}.switch input{width:1px;height:1px;opacity:0}.slider{position:absolute;inset:0;border-radius:99px;background:#c9c4bd;cursor:pointer;transition:.15s}.slider:after{content:"";position:absolute;width:19px;height:19px;left:3px;top:3px;border-radius:50%;background:#fff;box-shadow:0 1px 4px rgba(0,0,0,.22);transition:.15s}.switch input:checked+.slider{background:var(--primary)}.switch input:checked+.slider:after{transform:translateX(18px)}code{font:12px ui-monospace,SFMono-Regular,Menlo,monospace}.credentials{margin-top:14px;padding-top:13px;border-top:1px solid var(--line);color:var(--muted);font-size:12px}
    @media(max-width:580px){main{width:min(100% - 24px,680px);padding-top:25px}.states{grid-template-columns:1fr}.field-row,.status-meta{grid-template-columns:1fr}}
    @media(prefers-color-scheme:dark){:root{color-scheme:dark;--bg:#1d1b19;--card:#292623;--text:#f2eee9;--muted:#aaa39b;--line:#3d3934;--input:#59524b;--primary:#cf745d;--primary-hover:#dc856f;--soft:#3a2925;--shadow:none}.state,.validation{background:#211f1d}.state.remote{background:rgba(62,140,125,.18)}}
  </style>
</head>
<body>
<main>
  <header><h1>备份设置</h1><p class="subtitle">管理账本快照、本地提交和异地仓库同步。</p></header>

  <section class="card" aria-labelledby="config-title">
    <div class="card-head"><h2 id="config-title">备份仓库</h2></div>
    <form id="config-form">
      <div class="field"><label for="repoPath">仓库目录</label><input id="repoPath" name="repoPath" type="text" spellcheck="false" autocomplete="off" placeholder="/Users/you/ledger-backup"></div>
      <div class="field-row">
        <div class="field"><label for="remote">Remote</label><input id="remote" name="remote" type="text" spellcheck="false" value="origin"></div>
        <div class="field"><label for="branch">Branch</label><input id="branch" name="branch" type="text" spellcheck="false" value="main"></div>
      </div>
      <div id="validation" class="validation" role="status">正在读取配置…</div>
      <div class="save-row"><button id="save" type="submit">保存并校验</button><span id="save-feedback" class="feedback"></span></div>
    </form>
    <p class="credentials">应用不保存任何 git 凭据。推送使用你系统里的 SSH key 或 credential helper；请先在终端完成仓库和 remote 配置。</p>
  </section>

  <section class="card" aria-labelledby="status-title">
    <div class="card-head"><h2 id="status-title">备份状态</h2><span id="poll-state" class="help">每 2 秒刷新</span></div>
    <div class="states">
      <div class="state"><strong>快照已生成</strong><time id="snapshot-at">从未</time></div>
      <div class="state"><strong>本地已提交</strong><time id="commit-at">从未</time></div>
      <div class="state remote"><strong>远端已确认 · 异地备份完成</strong><time id="remote-at">从未</time></div>
    </div>
    <div class="status-meta">
      <div class="meta"><span>待推送提交数</span><strong id="unpushed">0</strong></div>
      <div class="meta"><span>最近本地提交</span><strong id="commit-id">无</strong></div>
    </div>
    <div id="unpushed-warning" class="warning">这些改动还没到远端，本机磁盘损坏会一起丢失。</div>
    <div id="last-error" class="error"></div>
    <div class="actions"><button id="run" type="button">立即备份</button><span id="running" class="running"></span></div>
  </section>

  <section class="card" aria-labelledby="auto-title">
    <div class="switch-row">
      <div><h2 id="auto-title">自动备份</h2><p class="help">成功写入账本后等待 30 秒；期间再次写入会重新计时。另有 15 分钟看门狗补漏。</p></div>
      <label class="switch" aria-label="启用自动备份"><input id="autoBackup" type="checkbox"><span class="slider"></span></label>
    </div>
  </section>
</main>
<script>
  const $ = id => document.getElementById(id);
  let currentConfig = null;

  function formatTime(value) {
    if (!value) return '从未';
    const date = new Date(value);
    return Number.isNaN(date.getTime()) ? value : date.toLocaleString('zh-CN', { hour12: false });
  }

  function showValidation(result) {
    const box = $('validation');
    box.className = 'validation ' + (result.valid ? 'good' : 'bad');
    box.innerHTML = `<div>${escapeHtml(result.message)}</div><div class="checks">
      <span class="check ${result.directoryExists ? 'ok' : ''}">目录${result.directoryExists ? '存在' : '不存在'}</span>
      <span class="check ${result.gitRepository ? 'ok' : ''}">${result.gitRepository ? '是 git 仓库' : '不是 git 仓库'}</span>
      <span class="check ${result.remoteExists ? 'ok' : ''}">remote ${result.remoteExists ? '已配置' : '未配置'}</span>
    </div>`;
  }

  function escapeHtml(value) {
    const element = document.createElement('div');
    element.textContent = String(value ?? '');
    return element.innerHTML;
  }

  async function request(url, options) {
    const response = await fetch(url, options);
    const body = await response.json();
    if (!response.ok) throw new Error(body.error || `请求失败（${response.status}）`);
    return body;
  }

  async function loadConfig() {
    try {
      const body = await request('/desktop/backup/api/config');
      currentConfig = body.config;
      $('repoPath').value = body.config.repoPath;
      $('remote').value = body.config.remote;
      $('branch').value = body.config.branch;
      $('autoBackup').checked = body.config.autoBackup;
      showValidation(body.validation);
    } catch (error) {
      $('validation').className = 'validation bad';
      $('validation').textContent = error.message;
    }
  }

  async function saveConfig(event) {
    event?.preventDefault();
    const button = $('save');
    button.disabled = true;
    $('save-feedback').textContent = '正在校验…';
    const config = {
      repoPath: $('repoPath').value.trim(), remote: $('remote').value.trim(),
      branch: $('branch').value.trim(), autoBackup: $('autoBackup').checked
    };
    try {
      const body = await request('/desktop/backup/api/config', {
        method: 'PUT', headers: {'Content-Type': 'application/json'}, body: JSON.stringify(config)
      });
      currentConfig = body.config;
      showValidation(body.validation);
      $('save-feedback').textContent = '已保存';
    } catch (error) {
      $('save-feedback').textContent = error.message;
      $('validation').className = 'validation bad';
      $('validation').textContent = error.message;
    } finally { button.disabled = false; }
  }

  async function loadStatus() {
    try {
      const status = await request('/desktop/backup/api/status');
      $('snapshot-at').textContent = formatTime(status.lastSnapshotAt);
      $('commit-at').textContent = formatTime(status.lastCommitAt);
      $('remote-at').textContent = formatTime(status.lastRemoteConfirmAt);
      $('unpushed').textContent = status.unpushedCommits;
      $('commit-id').textContent = status.lastCommitId ? status.lastCommitId.slice(0, 12) : '无';
      $('unpushed-warning').style.display = status.unpushedCommits > 0 ? 'block' : 'none';
      $('last-error').style.display = status.lastError ? 'block' : 'none';
      $('last-error').textContent = status.lastError || '';
      $('run').disabled = status.running;
      $('running').textContent = status.running ? '正在执行，请稍候…' : '';
      $('poll-state').textContent = '刚刚刷新';
    } catch (error) { $('poll-state').textContent = error.message; }
  }

  $('config-form').addEventListener('submit', saveConfig);
  $('autoBackup').addEventListener('change', () => { if (currentConfig) saveConfig(); });
  $('run').addEventListener('click', async () => {
    $('run').disabled = true; $('running').textContent = '正在启动…';
    try { await request('/desktop/backup/api/run', {method: 'POST'}); await loadStatus(); }
    catch (error) { $('running').textContent = error.message; $('run').disabled = false; }
  });
  loadConfig(); loadStatus(); setInterval(loadStatus, 2000);
</script>
</body>
</html>"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_keeps_three_backup_states_separate() {
        assert!(HTML.contains("快照已生成"));
        assert!(HTML.contains("本地已提交"));
        assert!(HTML.contains("远端已确认 · 异地备份完成"));
        assert!(HTML.contains("这些改动还没到远端"));
    }
}
