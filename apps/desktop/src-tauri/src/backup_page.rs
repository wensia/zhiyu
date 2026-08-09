pub const HTML: &str = r#"<!doctype html>
<html lang="zh-CN">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width,initial-scale=1">
  <title>知余 · 备份设置</title>
  <style>
    :root{color-scheme:light;--bg:#fbfaf8;--card:#fff;--text:#2f2f2f;--muted:#77716a;--line:#e8e3dc;--input:#d8d3cc;--primary:#b6533c;--primary-hover:#9f4532;--soft:#f7ece9;--success:#3e8c7d;--warning:#a86420;--danger:#b6533c;--shadow:0 10px 30px rgba(58,45,35,.06)}
    *{box-sizing:border-box}body{margin:0;background:var(--bg);color:var(--text);font:14px/1.55 -apple-system,BlinkMacSystemFont,"Segoe UI",sans-serif}main{width:min(680px,calc(100% - 36px));margin:0 auto;padding:38px 0 48px}header{margin-bottom:24px}h1{margin:0 0 5px;font-size:24px;letter-spacing:-.02em}h2{margin:0;font-size:16px}p{margin:0}.subtitle,.help{color:var(--muted)}.card{margin-top:14px;padding:20px;background:var(--card);border:1px solid var(--line);border-radius:14px;box-shadow:var(--shadow)}.card-head{display:flex;align-items:center;justify-content:space-between;gap:12px;margin-bottom:17px}.status-pill{padding:3px 9px;border:1px solid var(--line);border-radius:99px;color:var(--muted);font-size:12px;white-space:nowrap}.status-pill.good{border-color:rgba(62,140,125,.35);color:var(--success)}.status-pill.bad{border-color:rgba(182,83,60,.3);color:var(--danger)}.status-pill.warn{border-color:rgba(168,100,32,.3);color:var(--warning)}.notice{padding:12px 13px;border-radius:9px;background:var(--bg);color:var(--muted)}.notice.good{background:rgba(62,140,125,.1);color:var(--success)}.notice.bad{background:rgba(182,83,60,.09);color:var(--danger)}.notice.warn{background:rgba(190,124,50,.11);color:var(--warning)}.facts{display:flex;flex-wrap:wrap;gap:8px;margin-top:11px}.fact{padding:3px 8px;border:1px solid var(--line);border-radius:99px;color:var(--muted);font-size:12px}.command{display:none;margin-top:13px;padding:12px;background:var(--bg);border:1px solid var(--line);border-radius:9px}.command code{display:block;overflow:auto;margin-bottom:10px;white-space:nowrap}.actions,.inline-actions{display:flex;align-items:center;flex-wrap:wrap;gap:10px;margin-top:15px}.inline-actions{margin-top:10px}.field{margin-top:14px}.field:first-child{margin-top:0}label{display:block;margin-bottom:6px;font-size:13px;font-weight:600}input[type=text],select{width:100%;height:39px;padding:0 11px;color:var(--text);background:var(--card);border:1px solid var(--input);border-radius:8px;outline:none;font:inherit}input[type=text]:focus,select:focus{border-color:var(--primary);box-shadow:0 0 0 3px rgba(182,83,60,.13)}button,.button-link{display:inline-flex;height:38px;align-items:center;justify-content:center;padding:0 15px;border:0;border-radius:8px;background:var(--primary);color:#fff;font:600 14px/1 inherit;text-decoration:none;cursor:pointer}button.secondary,.button-link.secondary{background:var(--bg);border:1px solid var(--line);color:var(--text)}button:hover,.button-link:hover{background:var(--primary-hover)}button.secondary:hover,.button-link.secondary:hover{background:var(--soft)}button:focus-visible,.button-link:focus-visible{outline:3px solid rgba(182,83,60,.22);outline-offset:2px}button:disabled{opacity:.48;cursor:not-allowed}.feedback{color:var(--muted);font-size:13px}.setup-panel{display:none;margin-top:15px;padding-top:15px;border-top:1px solid var(--line)}.binding-title{font-weight:650}.binding-path{overflow-wrap:anywhere;margin-top:5px;color:var(--muted);font-size:12px}.details{margin-top:12px}.details summary{color:var(--muted);cursor:pointer}.details dl{display:grid;grid-template-columns:auto 1fr;gap:5px 12px;margin:10px 0 0}.details dt{color:var(--muted)}.details dd{min-width:0;margin:0;overflow-wrap:anywhere}.states{display:grid;grid-template-columns:repeat(3,1fr);gap:9px}.state{min-width:0;padding:13px;background:var(--bg);border-radius:9px}.state strong{display:block;margin-bottom:5px;font-size:12px}.state time{display:block;overflow:hidden;color:var(--muted);font-size:12px;text-overflow:ellipsis}.state.remote{background:rgba(62,140,125,.09)}.state.remote strong{color:var(--success)}.status-meta{display:grid;grid-template-columns:1fr 1fr;gap:10px;margin-top:12px}.meta{padding:11px 13px;border:1px solid var(--line);border-radius:9px}.meta span{display:block;color:var(--muted);font-size:12px}.meta strong{display:block;margin-top:3px}.warning,.error{display:none;margin-top:12px;padding:11px 12px;border-radius:8px}.warning{background:rgba(190,124,50,.11);color:var(--warning)}.error{white-space:pre-wrap;background:rgba(182,83,60,.09);color:var(--danger)}.running{color:var(--muted)}.switch-row{display:flex;align-items:flex-start;justify-content:space-between;gap:18px}.switch{position:relative;flex:none;width:43px;height:25px}.switch input{width:1px;height:1px;opacity:0}.slider{position:absolute;inset:0;border-radius:99px;background:#c9c4bd;cursor:pointer;transition:.15s}.slider:after{content:"";position:absolute;width:19px;height:19px;left:3px;top:3px;border-radius:50%;background:#fff;box-shadow:0 1px 4px rgba(0,0,0,.22);transition:.15s}.switch input:checked+.slider{background:var(--primary)}.switch input:checked+.slider:after{transform:translateX(18px)}.switch input:disabled+.slider{opacity:.48;cursor:not-allowed}code{font:12px ui-monospace,SFMono-Regular,Menlo,monospace}
    @media(max-width:580px){main{width:min(100% - 24px,680px);padding-top:25px}.states,.status-meta{grid-template-columns:1fr}.card{padding:17px}.card-head{align-items:flex-start}}
    @media(prefers-color-scheme:dark){:root{color-scheme:dark;--bg:#1d1b19;--card:#292623;--text:#f2eee9;--muted:#aaa39b;--line:#3d3934;--input:#59524b;--primary:#cf745d;--primary-hover:#dc856f;--soft:#3a2925;--shadow:none}.state,.notice,.command{background:#211f1d}.state.remote{background:rgba(62,140,125,.18)}}
  </style>
</head>
<body>
<main>
  <header><h1>备份设置</h1><p class="subtitle">把经过校验的账本快照同步到你的 GitHub 私有仓库。</p></header>

  <section class="card" aria-labelledby="github-title">
    <div class="card-head"><h2 id="github-title">1. GitHub CLI</h2><span id="gh-pill" class="status-pill">正在检测</span></div>
    <div id="gh-message" class="notice" role="status">正在检测本机 GitHub CLI 和登录状态…</div>
    <div id="gh-facts" class="facts"></div>
    <div id="install-command" class="command"><code id="install-code">brew install gh</code><div class="inline-actions"><button class="secondary" type="button" data-copy="install-code">复制安装命令</button><a class="button-link secondary" href="https://cli.github.com/" target="_blank" rel="noreferrer">查看官方安装说明</a></div></div>
    <div id="login-command" class="command"><code id="login-code">gh auth login --hostname github.com --git-protocol https --web --clipboard</code><div class="inline-actions"><button class="secondary" type="button" data-copy="login-code">复制登录命令</button></div></div>
    <div class="actions"><button id="refresh" class="secondary" type="button">重新检测</button><span id="copy-feedback" class="feedback" aria-live="polite"></span></div>
  </section>

  <section class="card" aria-labelledby="repo-title">
    <div class="card-head"><h2 id="repo-title">2. 私有备份仓库</h2><span id="repo-pill" class="status-pill">尚未绑定</span></div>
    <div id="repo-message" class="notice" role="status">GitHub CLI 就绪后，可以创建或选择私有仓库。</div>

    <div id="unconfigured-panel" class="setup-panel" style="display:block">
      <div class="field"><label for="repo-name">新仓库名称</label><input id="repo-name" type="text" value="zhiyu-backup" maxlength="100" spellcheck="false" autocomplete="off"></div>
      <div class="actions"><button id="create-repo" type="button" disabled>创建私有仓库</button><button id="show-existing" class="secondary" type="button" disabled>选择已有私有仓库</button><span id="repo-feedback" class="feedback" aria-live="polite"></span></div>
      <div id="existing-panel" class="setup-panel">
        <div class="field"><label for="repository-list">可写的私有仓库</label><select id="repository-list"><option value="">正在读取…</option></select></div>
        <div class="actions"><button id="bind-repo" type="button" disabled>绑定所选仓库</button></div>
      </div>
    </div>

    <div id="bound-panel" class="setup-panel">
      <div id="bound-name" class="binding-title"></div>
      <div id="bound-path" class="binding-path"></div>
      <details class="details"><summary>查看本地 Git 详情</summary><dl><dt>Remote</dt><dd id="bound-remote">origin</dd><dt>Branch</dt><dd id="bound-branch">main</dd></dl></details>
      <div id="restore-actions" class="actions"><button id="restore-repo" type="button">从该备份恢复</button><span id="restore-feedback" class="feedback">恢复将在下次启动时离线执行。</span></div>
    </div>
  </section>

  <section class="card" aria-labelledby="status-title">
    <div class="card-head"><h2 id="status-title">3. 备份状态</h2><span id="poll-state" class="help">每 2 秒刷新</span></div>
    <div class="states">
      <div class="state"><strong>快照已生成</strong><time id="snapshot-at">从未</time></div>
      <div class="state"><strong>本地已提交</strong><time id="commit-at">从未</time></div>
      <div class="state remote"><strong>远端已确认 · 异地备份完成</strong><time id="remote-at">从未</time></div>
    </div>
    <div class="status-meta"><div class="meta"><span>待推送提交数</span><strong id="unpushed">0</strong></div><div class="meta"><span>最近本地提交</span><strong id="commit-id">无</strong></div></div>
    <div id="unpushed-warning" class="warning">这些改动还没到远端，本机磁盘损坏会一起丢失。</div>
    <div id="last-error" class="error" role="alert"></div>
    <div class="actions"><button id="run" type="button" disabled>立即同步备份</button><span id="running" class="running" aria-live="polite"></span></div>
  </section>

  <section class="card" aria-labelledby="auto-title">
    <div class="switch-row"><div><h2 id="auto-title">自动同步</h2><p id="auto-help" class="help">正在检查 GitHub CLI 和私有仓库绑定状态…</p></div><label class="switch" aria-label="启用自动同步"><input id="autoBackup" type="checkbox" disabled><span class="slider"></span></label></div>
  </section>
</main>
<script>
  const $ = id => document.getElementById(id);
  let currentConfig = null;
  let currentSetup = null;

  function formatTime(value) {
    if (!value) return '从未';
    const date = new Date(value);
    return Number.isNaN(date.getTime()) ? value : date.toLocaleString('zh-CN', {hour12:false});
  }

  async function request(url, options) {
    const response = await fetch(url, options);
    const body = await response.json();
    if (!response.ok) throw new Error(body.error || `请求失败（${response.status}）`);
    return body;
  }

  function setPill(element, text, tone) {
    element.textContent = text;
    element.className = 'status-pill' + (tone ? ` ${tone}` : '');
  }

  function renderSetup(body) {
    currentSetup = {githubCapability:body.githubCapability, repositoryBinding:body.repositoryBinding, syncEnabled:body.syncEnabled};
    const gh = body.githubCapability;
    const ghTone = gh.state === 'ready' ? 'good' : gh.state === 'unauthenticated' ? 'warn' : 'bad';
    setPill($('gh-pill'), gh.state === 'ready' ? '已就绪' : gh.state === 'missing' ? '未安装' : gh.state === 'unauthenticated' ? '未登录' : '检测失败', ghTone);
    $('gh-message').className = `notice ${ghTone}`;
    $('gh-message').textContent = gh.message;
    $('gh-facts').innerHTML = '';
    for (const value of [gh.account && `账号：${gh.account}`, gh.version]) {
      if (!value) continue;
      const fact = document.createElement('span'); fact.className = 'fact'; fact.textContent = value; $('gh-facts').appendChild(fact);
    }
    $('install-command').style.display = gh.state === 'missing' ? 'block' : 'none';
    $('login-command').style.display = ['unauthenticated','error'].includes(gh.state) ? 'block' : 'none';
    if (gh.installCommand) $('install-code').textContent = gh.installCommand;
    if (gh.loginCommand) $('login-code').textContent = gh.loginCommand;

    const binding = body.repositoryBinding;
    const repoTone = binding.state === 'ready' ? 'good' : binding.state === 'restoreRequired' ? 'warn' : binding.state === 'invalid' ? 'bad' : '';
    setPill($('repo-pill'), binding.state === 'ready' ? '已绑定' : binding.state === 'restoreRequired' ? '等待恢复' : binding.state === 'invalid' ? '配置不可用' : '尚未绑定', repoTone);
    $('repo-message').className = `notice ${repoTone}`;
    $('repo-message').textContent = binding.message;
    const needsBinding = ['unconfigured','invalid'].includes(binding.state);
    const canConfigure = gh.state === 'ready' && needsBinding;
    $('unconfigured-panel').style.display = needsBinding ? 'block' : 'none';
    $('bound-panel').style.display = ['ready','restoreRequired'].includes(binding.state) ? 'block' : 'none';
    $('bound-name').textContent = binding.nameWithOwner ? `${binding.nameWithOwner} · 私有仓库` : '';
    $('bound-path').textContent = binding.repoPath || '';
    $('restore-actions').style.display = binding.state === 'restoreRequired' ? 'flex' : 'none';
    $('autoBackup').checked = Boolean(currentConfig?.autoBackup && binding.state === 'ready');
    $('create-repo').disabled = !canConfigure;
    $('show-existing').disabled = !canConfigure;
    $('run').disabled = !body.syncEnabled;
    $('autoBackup').disabled = !body.syncEnabled;
    $('auto-help').textContent = body.syncEnabled ? '成功写入账本后等待 30 秒；另有 15 分钟看门狗补漏。' : '完成 GitHub 登录和私有仓库绑定后才能启用。';
  }

  async function loadConfig() {
    $('refresh').disabled = true;
    try {
      const body = await request('/desktop/backup/api/config');
      currentConfig = body.config;
      $('autoBackup').checked = body.config.autoBackup;
      $('bound-remote').textContent = body.config.remote;
      $('bound-branch').textContent = body.config.branch;
      renderSetup(body);
    } catch (error) {
      $('gh-message').className = 'notice bad'; $('gh-message').textContent = error.message;
    } finally { $('refresh').disabled = false; }
  }

  async function loadStatus() {
    try {
      const status = await request('/desktop/backup/api/status');
      $('snapshot-at').textContent = formatTime(status.lastSnapshotAt);
      $('commit-at').textContent = formatTime(status.lastCommitAt);
      $('remote-at').textContent = formatTime(status.lastRemoteConfirmAt);
      $('unpushed').textContent = status.unpushedCommits;
      $('commit-id').textContent = status.lastCommitId ? status.lastCommitId.slice(0,12) : '无';
      $('unpushed-warning').style.display = status.unpushedCommits > 0 ? 'block' : 'none';
      $('last-error').style.display = status.lastError ? 'block' : 'none';
      $('last-error').textContent = status.lastError || '';
      $('run').disabled = status.running || !currentSetup?.syncEnabled;
      $('running').textContent = status.running ? '正在生成、提交并确认远端备份…' : '';
      $('poll-state').textContent = '刚刚刷新';
    } catch (error) { $('poll-state').textContent = error.message; }
  }

  async function withRepoAction(button, loading, action) {
    button.disabled = true; $('repo-feedback').textContent = loading;
    try { await action(); await loadConfig(); $('repo-feedback').textContent = ''; }
    catch (error) { $('repo-feedback').textContent = error.message; }
    finally { button.disabled = false; }
  }

  $('refresh').addEventListener('click', loadConfig);
  $('create-repo').addEventListener('click', () => withRepoAction($('create-repo'), '正在创建并绑定私有仓库…', async () => {
    await request('/desktop/backup/api/github/create', {method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({name:$('repo-name').value.trim()})});
  }));
  $('show-existing').addEventListener('click', () => withRepoAction($('show-existing'), '正在读取私有仓库…', async () => {
    const body = await request('/desktop/backup/api/github/repositories');
    const select = $('repository-list'); select.innerHTML = '';
    if (!body.repositories.length) { const option = document.createElement('option'); option.textContent = '没有可写的私有仓库'; option.value = ''; select.appendChild(option); }
    for (const repo of body.repositories) { const option = document.createElement('option'); option.value = repo.nameWithOwner; option.textContent = `${repo.nameWithOwner}${repo.isEmpty ? ' · 空仓库' : ' · 将先校验知余备份'}`; select.appendChild(option); }
    $('existing-panel').style.display = 'block'; $('bind-repo').disabled = !select.value;
  }));
  $('repository-list').addEventListener('change', () => { $('bind-repo').disabled = !$('repository-list').value; });
  $('bind-repo').addEventListener('click', () => withRepoAction($('bind-repo'), '正在克隆并校验仓库…', async () => {
    await request('/desktop/backup/api/github/bind', {method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({nameWithOwner:$('repository-list').value})});
  }));
  $('restore-repo').addEventListener('click', async () => {
    const button = $('restore-repo'); button.disabled = true; $('restore-feedback').textContent = '正在保存恢复请求…';
    try { const body = await request('/desktop/backup/api/restore', {method:'POST'}); $('restore-feedback').textContent = body.message; }
    catch (error) { $('restore-feedback').textContent = error.message; button.disabled = false; }
  });
  $('autoBackup').addEventListener('change', async event => {
    const enabled = event.target.checked; event.target.disabled = true;
    try { await request('/desktop/backup/api/auto-backup', {method:'PUT',headers:{'Content-Type':'application/json'},body:JSON.stringify({enabled})}); }
    catch (error) { event.target.checked = !enabled; $('auto-help').textContent = error.message; }
    finally { event.target.disabled = !currentSetup?.syncEnabled; }
  });
  $('run').addEventListener('click', async () => {
    $('run').disabled = true; $('running').textContent = '正在启动同步…';
    try { await request('/desktop/backup/api/run', {method:'POST'}); await loadStatus(); }
    catch (error) { $('running').textContent = error.message; $('run').disabled = !currentSetup?.syncEnabled; }
  });
  document.querySelectorAll('[data-copy]').forEach(button => button.addEventListener('click', async () => {
    const text = $(button.dataset.copy).textContent;
    try { await navigator.clipboard.writeText(text); $('copy-feedback').textContent = '命令已复制'; }
    catch (_) { const input = document.createElement('textarea'); input.value = text; document.body.appendChild(input); input.select(); document.execCommand('copy'); input.remove(); $('copy-feedback').textContent = '命令已复制'; }
  }));

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

    #[test]
    fn page_has_actionable_github_setup_states() {
        for text in [
            "brew install gh",
            "复制登录命令",
            "创建私有仓库",
            "选择已有私有仓库",
            "从该备份恢复",
        ] {
            assert!(HTML.contains(text), "missing {text}");
        }
        assert!(HTML.contains("const needsBinding"));
        assert!(HTML.contains("needsBinding ? 'block' : 'none'"));
        assert!(
            HTML.contains(
                "id=\"unconfigured-panel\" class=\"setup-panel\" style=\"display:block\""
            )
        );
    }
}
