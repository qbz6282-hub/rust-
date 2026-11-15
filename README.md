# coding_tools · 多後端 LLM 對話與沙箱工具

一個用 Rust 寫的命令列對話程式。同一份程式可切換 **OpenRouter（雲端）**、**Ollama（本機）**、**LM Studio（本機）**，並可選擇是否讓模型呼叫工具、在 Podman 沙箱裡執行 Python / Rust 程式碼。

> 第一版是一位高職二年級資訊科學生在沒有 AI 協助下寫成的；之後陸續加入統一適配器、工具能力偵測與沙箱。

---

## 目錄結構

| 路徑 | 內容 |
| --- | --- |
| `src/main.rs` | 對話主迴圈、工具呼叫、Podman 沙箱 |
| `src/adapter.rs` | 多後端統一適配器：端點、標頭、格式正規化、工具能力 |
| `Cargo.toml` / `Cargo.lock` | 依賴與版本鎖定 |
| `config.example.json` | 設定範本（請複製成 `config.json` 再填） |
| `docs/` | 開發紀錄、原始說明 |
| `archive/` | 早期版本原始碼，**僅供對照，不可編譯** |
| `examples/` | 模型在沙箱裡產生的範例腳本 |

## 快速開始（Windows 11 / PowerShell）

```powershell
Copy-Item config.example.json config.json
cargo run
```

`config.json` 已列在 `.gitignore`，不會被推上 GitHub。直接輸入文字開始對話，輸入 `跑路` 離開。

## 三種後端

| 後端 | 預設端點 | 鑑權 | 工具能力判定 |
| --- | --- | --- | --- |
| OpenRouter | `https://openrouter.ai/api/v1/chat/completions` | `Bearer` + API Key | 無查詢 API，保守視為支援 |
| Ollama | `http://localhost:11434/v1/chat/completions` | 佔位 `Bearer ollama` | 查 `/api/show` 的 `capabilities` 是否含 `tools` |
| LM Studio | `http://localhost:1234/v1/chat/completions` | 佔位 `Bearer lm-studio` | 無查詢 API，保守視為不支援 |

Ollama 另可用 `OLLAMA_NATIVE=1` 切到原生 `/api/chat` 格式。

## 工具請求開關（`tools`）

| 值 | 行為 |
| --- | --- |
| `auto` | 開程式／換後端／換模型時自動查能力，支援才送 `tools`（建議） |
| `on` | 強制送；不支援的模型會回 `400 does not support tools` |
| `off` | 完全不送，連歷史裡的工具訊息也會過濾掉，純文字對話 |

`auto` 查不到時採保守值：Ollama、LM Studio 不送，OpenRouter 送。關閉工具時 Podman 沙箱不會被觸發。

## 對話中可用的指令

| 輸入 | 作用 |
| --- | --- |
| `後端 openrouter` / `ollama` / `lmstudio` | 切換後端 |
| `模型 <名稱>` | 換模型並重新偵測能力 |
| `工具 on` / `off` / `auto` / `重探` | 調整工具策略 |
| `工具` 或 `後端` | 顯示目前狀態 |
| `清空` | 清空對話紀錄 |
| `跑路` / `滾木` | 結束程式 |

設定優先順序：對話指令 > 環境變數 > `config.json` > 內建預設。可用環境變數包含 `AI_BACKEND`、`AI_TOOLS`、`AI_TIMEOUT_SECS`、各後端的 `*_MODEL` / `*_HOST` / `*_TOOLS`，以及 `OPENROUTER_API_KEY`。

## 環境需求

- Rust（edition 2024）
- 三个後端擇一：本機裝 Ollama 或 LM Studio，或申請 OpenRouter API Key
- **Podman**：Windows 11 家用版沒有 Hyper-V，需啟用「虛擬機器平台」與 WSL2，並先啟動 Podman Machine，`podman run` 才會動

## 版本沿革

| 版本 | 位置 | 說明 |
| --- | --- | --- |
| v1 | `archive/v1_first_draft.rs.txt` | 最初的對話程式，無 AI 協助 |
| v2 | `archive/v2_sandbox_draft.rs.txt` | 加入 `tools` 與 Podman 沙箱，未實測 |
| v3 | `archive/v3_main.rs.txt`、`v3_adapter.rs.txt` | 另一分支的適配器版本 |
| 現行 | `src/` | 三後端統一適配器 + 工具能力偵測，可編譯、有單元測試 |

詳細緣由見 `docs/開發紀錄.md`。`archive/RWKV6.rs.txt` 與 `examples/model_generated_multiply.py` 是當時模型在沙箱裡生成的產物，獨立保留。

## ⚠️ 安全提醒

早期版本曾把 API Key 直接寫在原始碼與 `config.json` 並推上公開儲存庫。**那些舊金鑰請到 <https://openrouter.ai/keys> 撤銷**；本整理版已把歸檔檔案中的金鑰字串抹除。往後請勿上傳 `config.json` 或任何含 `sk-or-v1-` 的檔案。
