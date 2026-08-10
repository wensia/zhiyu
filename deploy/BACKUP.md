# 备份

三条互不依赖的路径，任何一条失效都不影响其余两条。

```
服务端 /data/backups/              每日快照，保留 30 天         应用内 tokio 定时任务
        │
        ├──▶ 本机 ~/zhiyu-backups/remote/   每日两次              launchd，不需要打开任何应用
        │
        └──▶ 桌面应用 app_data_dir/backups/  每 6 小时            仅在应用运行时
```

快照由服务端统一生成（`VACUUM INTO` + `integrity_check` + `foreign_key_check`），
另外两处只负责下载、校验、落盘与各自的保留策略。保留规则是共享的纯函数
（`crates/backup-policy`），但删除动作各自执行——服务器磁盘写满不应连带删掉本机
唯一的副本。

## 本机定时拉取

```bash
# 1. 在服务器上签发一把 api-key
docker exec zhiyu zhiyu-api-key <你的账号邮箱>

# 2. 凭证写入独立文件（不要放进 plist：它是 644 且会被 Spotlight 索引）
cat > ~/.zhiyu-backup.env <<'ENV'
ZHIYU_SERVER_URL=https://your-server.example.com
ZHIYU_API_KEY=<上一步得到的密钥>
ENV
chmod 600 ~/.zhiyu-backup.env

# 3. 安装定时任务
cp deploy/net.askfish.zhiyu.backup.plist.example \
   ~/Library/LaunchAgents/com.example.zhiyu.backup.plist
# 按实际情况修改其中的 Label 与脚本路径
launchctl load ~/Library/LaunchAgents/com.example.zhiyu.backup.plist

# 4. 立刻验证一次
launchctl start com.example.zhiyu.backup
tail -5 /tmp/zhiyu-backup.log
```

选 launchd 而非 cron：合盖睡眠期间错过的任务，launchd 在唤醒后会补跑，cron 直接跳过。
对笔记本这个差别很关键。

## 恢复

拉回来的就是普通 SQLite 文件，不需要任何专用工具：

```bash
sqlite3 ~/zhiyu-backups/remote/zhiyu-<时间戳>.db 'PRAGMA integrity_check;'
# 确认 ok 后停服务、替换 /opt/zhiyu/data/preview.db、重启
```

替换前务必把现有库连同 `-wal` / `-shm` 整组移走而不是就地覆盖：旧的 journal
与新主库错配可能直接损坏数据。

## 脚本自身的校验

`scripts/pull-remote-backup.sh` 每份下载都过三关：长度、SHA-256、以及再跑一次
`PRAGMA integrity_check`。最后一关不是多余——校验和相符也可能是一份内部已损坏的
库被完整地传输过来。任何一关不过就删掉临时文件并以非零码退出，绝不静默。
