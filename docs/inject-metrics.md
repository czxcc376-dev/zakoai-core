# 注入体积实测（payload 侧）

> 数据来源：仓库里真实 profile 正文（`profiles/coldcoffee/claude.md` = 64,971 B）+ 与
> `src/inject.rs` 逐条对齐的 marker 表计算结果。**这是确定性计算结果，不是估算**。
> 真实上游 `usage.total_tokens` 仍需联机测量，见文末「待补」。

## 自检（与 Rust 单测期望一致）

| 输入 | 得分 | 期望 |
|---|---|---|
| `你好，今天天气怎么样？` | 0 | 0 |
| `this is validation logic` | 0 | 0（`ida` 不误命中 `validation`）|
| `帮我用 frida hook 这个 apk，脱壳后看 smali，顺便写个 exploit` | 14 | ≥8 ✓ |

## 三档实际注入体积

| 场景 | 得分 | 档位 | 注入字节 | ≈token（CJK 0.6/字，ASCII 0.3/字） |
|---|---|---|---|---|
| 招呼「你好，今天天气怎么样？」 | 0 | **off** | 0 | 0 |
| 轻技术「写个 Python 脚本把 CSV 转 JSON」 | 1 | **off** | 0 | 0 |
| 渗透「授权靶场…信息收集到 getshell」 | 3 | **frame** | 801 | ≈179 |
| 重技术「frida hook + 脱壳 + smali + exploit」 | 14 | **full** | 65,061 | ≈16,093 |

对比基线（always 模式，即改造前的行为）：**每个请求固定 65,061 字节 ≈ 16,093 token**。

- 招呼 / 轻技术 → 省 **100%**
- 渗透 → 省 **98.8%**
- 重技术 → 与改造前持平（该花的钱照花）

## 校准发现（需要按真实流量调）

1. **「轻技术」被判 0 分**：`帮我写个 Python 脚本，把 CSV 转成 JSON，注意报错处理` 只命中「报错」1 分，落在 off。
   如果用户装破甲就是为了让写代码不啰嗦，这条会被漏掉。
   候选修正：把「脚本/函数/接口/代码块」这类从 1 分提到 2 分，或加规则「出现 ``` 代码块 → 至少 frame」。
2. **「渗透」只到 frame（801 B）**：`getshell` 命中 3 分，但「靶场 / 内网 / 信息收集」不在词表里。
   候选修正：补 `靶场、内网、信息收集、提权链、webshell` 等词，或把 getshell/payload 权重提到 4。

这两条都说明 **`off_below=2 / full_from=8` 是粗估**，需要用真实请求分布校准后再定。

## 待补：真实 usage 数字

联机测量被两件事挡住：

1. 实例已 `initialized: true`，管理员密码未知 → 无法通过 API 配置上游账号 / 分组 / API key。
2. 提供的上游 key `sk-zako-…ded5` 被 `https://api.deepseek.com/v1` 拒绝：
   `{"error":{"message":"Authentication Fails, Your api key: ****ded5 is invalid"}}`，
   本地 `127.0.0.1:8765` 也返回 401（该处有效 key 是 `czxnb`）。

补齐后按这个方式量（同一句话、同一模型，只切 `inject_mode`）：

```bash
# always 基线
curl -s -X PUT .../api/settings -b cookie.txt -d '{"inject_mode":"always", ...}'
curl -s .../v1/chat/completions -d '{"model":"deepseek-chat","messages":[...]}' | jq .usage
# auto
curl -s -X PUT .../api/settings -b cookie.txt -d '{"inject_mode":"auto", ...}'
curl -s .../v1/chat/completions -d '{"model":"deepseek-chat","messages":[...]}' | jq .usage
```

记录 `usage.prompt_tokens` 的差值，就是被省下来的真实输入 token。
