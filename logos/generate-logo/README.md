# generate-logo skill 产出（待 API key）

`IMAGEROUTER_API_KEY` 当前未设置，无法直接调用 ImageRouter API。
设置环境变量后，对本目录下两个 prompt 各执行一次即可生成 logo：

```bash
export IMAGEROUTER_API_KEY=...   # 从 https://imagerouter.io/api-keys 获取

for p in prompt-a prompt-b; do
  curl -s 'https://api.imagerouter.io/v1/openai/images/generations' \
    -H "Authorization: Bearer $IMAGEROUTER_API_KEY" \
    -H 'Content-Type: application/json' \
    -d "$(jq -n --arg prompt "$(cat $p.txt)" '{
          prompt: $prompt,
          model: "google/nano-banana-2",
          quality: "high",
          size: "1024x1024",
          response_format: "url",
          output_format: "png"
        }')" | jq -r '.data[0].url' \
  | xargs curl -s -o "$p.png"
done
```

产出后按 skill 工作流：`logo-v*.png` 比较挑选，最佳者复制为 `logo.png`。
