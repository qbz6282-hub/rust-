mod adapter;

use base64::{engine::general_purpose, Engine as _};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::borrow::Cow;
use std::error::Error;
use std::io::Write;

use adapter::{建立適配器, 後端種類, 後端適配器, 工具策略};

// 定義專用的 Result 型別，以兼容 tokio 的非同步錯誤
pub(crate) type 結局<T> = Result<T, Box<dyn Error + Send + Sync>>;

// ================= 結構體定義（全面支援零拷貝借用） =================

#[derive(Serialize, Debug)]
pub(crate) struct 對話請求<'q> {
    model: Cow<'q, str>,
    messages: &'q [Message<'q>],
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<&'q [Value]>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
struct Message<'q> {
    #[serde(borrow)]
    role: Cow<'q, str>,
    // 升級：將 content 也改為 Cow，讓零拷貝徹底貫徹到模型的回覆內文！
    #[serde(borrow, default, skip_serializing_if = "Option::is_none")]
    content: Option<Cow<'q, str>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<ToolCall>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<String>,
}

impl<'q> Message<'q> {
    fn 起會(角色: impl Into<Cow<'q, str>>, 內容: impl Into<Cow<'q, str>>) -> Self {
        Self {
            role: 角色.into(),
            content: Some(內容.into()),
            tool_calls: None,
            tool_call_id: None,
        }
    }

    /// 關鍵轉換方法：當這輪對話結束，需要把借用自網路緩衝區的臨時訊息，
    /// 轉化為擁有獨立所有權的 'static 訊息，才能安全存入永久的 `上下文` Vec 中
    fn 轉為永久所有權(self) -> Message<'static> {
        Message {
            role: Cow::Owned(self.role.into_owned()),
            content: self.content.map(|c| Cow::Owned(c.into_owned())),
            tool_calls: self.tool_calls,
            tool_call_id: self.tool_call_id,
        }
    }
}

#[derive(Deserialize, Debug)]
struct 對話響應<'q> {
    #[serde(borrow)]
    choices: Vec<Choice<'q>>,
}

#[derive(Deserialize, Debug)]
struct Choice<'q> {
    #[serde(borrow, rename = "message")]
    訊息: Message<'q>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
struct ToolCall {
    id: String,
    #[serde(rename = "type")]
    call_type: String,
    function: FunctionCall,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
struct FunctionCall {
    name: String,
    arguments: String,
}

// ================= 後端選擇 =================

/// 優先順序：命令列第一個參數 > AI_BACKEND > config.json 的 backend > 預設 ollama
fn 讀取後端種類() -> 後端種類 {
    let 指定 = std::env::args()
        .nth(1)
        .unwrap_or_else(|| adapter::設定值("", "backend", "AI_BACKEND", "ollama"));

    match 後端種類::解析(&指定) {
        Some(種類) => 種類,
        None => {
            eprintln!(
                "未知後端 '{}'，可用：{}；已改用 ollama",
                指定,
                後端種類::選單()
            );
            後端種類::LMStudio
        }
    }
}

fn 印出後端資訊(適配器: &dyn 後端適配器) {
    println!("[系統] 後端: {} | 格式: {}", 適配器.名稱(), 適配器.格式());
    println!("[系統] 端點: {}", 適配器.端點());
    println!("[系統] 模型: {}", 適配器.模型());
    println!("[系統] 工具請求: {}", 適配器.工具設定().狀態());
}

/// 統計上下文中殘留的工具訊息，切換為純文字模式時用來提醒
fn 工具痕跡數(上下文: &[Message<'static>]) -> usize {
    上下文
        .iter()
        .filter(|m| {
            m.role == "tool" || m.tool_call_id.is_some() || m.tool_calls.is_some()
        })
        .count()
}

// ================= 核心邏輯 =================

#[tokio::main]
async fn main() -> 結局<()> {
    let 用戶 = Client::new();
    let mut 上下文: Vec<Message<'static>> = Vec::new();
    let mut 適配器 = 建立適配器(讀取後端種類())?;
    // auto 模式會向後端查一次模型能力（Ollama 用 /api/show）
    adapter::預備工具能力(&用戶, &mut *適配器).await;

    let 利器 = vec![json!({
        "type": "function",
        "function": {
            "name": "run_code",
            "description": "在隔離的 Podman 沙箱中執行程式碼。支援 python 與 rust 兩種語言。",
            "parameters": {
                "type": "object",
                "properties": {
                    "語言": {
                        "type": "string",
                        "enum": ["python", "rust"],
                        "description": "欲執行之程式語言"
                    },
                    "代碼": {
                        "type": "string",
                        "description": "完整之程式碼字串"
                    }
                },
                "required": ["語言", "代碼"]
            }
        }
    })];

    println!("?!善善!? === 多後端統一適配器 · 零拷貝滾木沙箱模型 === ?!善善!?");
    印出後端資訊(&*適配器);
    println!("(輸入 '跑路' 或 '滾木' 離開程式)");
    println!("指令: '後端 <{}>' · '模型 <名稱>' · '工具 on|off|auto|重探' · '清空' · '後端' 查看現況", 後端種類::選單());

    loop {
        print!("\n尔言:\n");
        std::io::stdout().flush()?;
        let mut 輸入 = String::new();
        std::io::stdin().read_line(&mut 輸入)?;
        let 提示詞 = 輸入.trim();

        if 提示詞.is_empty() || 提示詞 == "跑路" || 提示詞 == "滾木" {
            break;
        }

        // ---- REPL 控制指令（不送進模型） ----
        if 提示詞 == "後端" || 提示詞 == "工具" {
            印出後端資訊(&*適配器);
            continue;
        }
        if let Some(名) = 提示詞.strip_prefix("後端 ") {
            match 後端種類::解析(名) {
                Some(種類) => match 建立適配器(種類) {
                    Ok(新適配器) => {
                        適配器 = 新適配器;
                        adapter::預備工具能力(&用戶, &mut *適配器).await;
                        印出後端資訊(&*適配器);
                    }
                    Err(e) => eprintln!("[錯誤] 無法建立適配器: {}", e),
                },
                None => eprintln!("可用後端：{}", 後端種類::選單()),
            }
            continue;
        }
        if let Some(名) = 提示詞.strip_prefix("模型 ") {
            if 名.trim().is_empty() {
                eprintln!("請輸入模型名稱，例如：模型 qwen2.5vl:7b");
            } else {
                適配器.改模型(名.trim().to_string());
                adapter::重探工具能力(&用戶, &mut *適配器).await;
                println!("[系統] 模型已設為 {} | 工具請求: {}", 適配器.模型(), 適配器.工具設定().狀態());
            }
            continue;
        }
        if 提示詞 == "清空" {
            let 筆數 = 上下文.len();
            上下文.clear();
            println!("[系統] 已清空 {} 則對話紀錄", 筆數);
            continue;
        }
        if 提示詞 == "工具 重探" {
            adapter::重探工具能力(&用戶, &mut *適配器).await;
            println!("[系統] 工具請求: {}", 適配器.工具設定().狀態());
            continue;
        }
        if let Some(值) = 提示詞.strip_prefix("工具 ") {
            match 工具策略::解析(值) {
                Some(策略) => {
                    適配器.設工具策略(策略);
                    adapter::預備工具能力(&用戶, &mut *適配器).await;
                    println!("[系統] 工具請求: {}", 適配器.工具設定().狀態());
                    if !適配器.送出工具() && 工具痕跡數(&上下文) > 0 {
                        println!(
                            "[提示] 上下文仍有 {} 則工具訊息（送出前會自動過濾）；建議輸入「清空」重開對話",
                            工具痕跡數(&上下文)
                        );
                    }
                }
                None => eprintln!("用法：工具 on | off | auto | 重探"),
            }
            continue;
        }

        上下文.push(Message::起會("user", 提示詞.to_string()));

        // Agent 內部迴圈
        loop {
            // 模型識別字由適配器決定；tools 一律先給，是否真的送出由傳輸層依工具策略把關
            let 請求資訊 = 對話請求 {
                model: Cow::Borrowed(適配器.模型()),
                messages: &上下文,
                tools: Some(&利器),
            };

            // 寄柬：統一傳輸層只把原始網路緩衝區 (Bytes) 撈回來，生命週期由當前迴圈掌控
            match adapter::寄柬(&用戶, &*適配器, &請求資訊).await {
                Ok(正規化緩衝區) => {
                    // 直接從緩衝區切片進行零拷貝反序列化！
                    // 若該後端格式無需改寫，這裡拿到的就是未經複製的原始 bytes。
                    let 響應: 對話響應 = match serde_json::from_slice(&正規化緩衝區) {
                        Ok(r) => r,
                        Err(e) => {
                            eprintln!(
                                "[解析錯誤] JSON 反序列化失敗: {}\n原始回傳: {}",
                                e,
                                String::from_utf8_lossy(&正規化緩衝區)
                            );
                            上下文.pop();
                            break;
                        }
                    };

                    let Some(首選) = 響應.choices.into_iter().next() else {
                        println!("[警告] {} 未回傳任何有效選項", 適配器.名稱());
                        break;
                    };
                    let 模型訊息 = 首選.訊息;
                    let 允許工具 = 適配器.送出工具();

                    // 純文字模式下，即使模型幻覺出 tool_calls 也不啟動 Podman
                    if let Some(工具集) = 模型訊息.tool_calls.as_ref().filter(|組| 允許工具 && !組.is_empty()) {
                        上下文.push(模型訊息.clone().轉為永久所有權());

                        for 工具 in 工具集 {
                            println!("[系統] {} 請求執行工具: {}", 適配器.名稱(), 工具.function.name);
                            let 執行結果 = run_code(&工具.function).await;

                            let 工具回覆 = Message {
                                role: Cow::Borrowed("tool"),
                                content: Some(Cow::Owned(執行結果)),
                                tool_calls: None,
                                tool_call_id: Some(工具.id.clone()),
                            };
                            上下文.push(工具回覆);
                        }
                        // 迴圈於此處進入下一輪，而上一輪的緩衝區在此處被安全釋放
                        continue;
                    }

                    // 列印模型回覆時，直接讀取借用自緩衝區的指標，完全沒有發生任何字串複製
                    if let Some(內容) = &模型訊息.content {
                        println!("模型:\n{}", 內容);
                    }
                    if 模型訊息.tool_calls.is_some() && !允許工具 {
                        println!("[提示] 工具呼叫已停用，忽略模型回傳的 tool_calls（純文字模式）");
                    }

                    上下文.push(模型訊息.轉為永久所有權());
                    break;
                }
                Err(e) => {
                    println!("[錯誤] 請求失敗: {}", e);
                    上下文.pop();
                    break;
                }
            }
        }
    }
    Ok(())
}

/// 解析參數並啟動 Podman 沙箱
async fn run_code(術: &FunctionCall) -> String {
    let 參數: Value = match serde_json::from_str(&術.arguments) {
        Ok(v) => v,
        Err(_) => return "參數解析失敗".to_string(),
    };

    let 語言 = 參數["語言"].as_str().unwrap_or("python");
    let 代碼 = 參數["代碼"].as_str().unwrap_or("");

    let 密文 = general_purpose::STANDARD.encode(代碼);

    let (映像, 容器內指令) = if 語言 == "python" {
        (
            "docker.io/library/python:3.12-slim",
            format!("echo '{}' | base64 -d > /tmp/main.py && python /tmp/main.py", 密文),
        )
    } else {
        (
            "docker.io/library/rust:1-slim-bookworm",
            format!("cargo init --name 沙箱專案 -q /tmp/rust_zuan1an4 && \
                 echo '{}' | base64 -d > /tmp/rust_zuan1an4/src/main.rs && \
                 cd /tmp/rust_zuan1an4 && cargo run", 密文),
        )
    };

    match tokio::process::Command::new("podman")
        .args(["run", "--rm", 映像, "bash", "-c", &容器內指令])
        .output()
        .await
    {
        Ok(輸出) => {
            let 標準輸出 = String::from_utf8_lossy(&輸出.stdout);
            let 標準錯誤 = String::from_utf8_lossy(&輸出.stderr);
            let 狀態碼 = 輸出.status.code().unwrap_or(-1);

            if 狀態碼 == 0 {
                format!("執行成功 (狀態碼 0):\n{}", 標準輸出)
            } else {
                format!("執行失敗 (狀態碼 {}):\n標準輸出:\n{}\n標準錯誤:\n{}", 狀態碼, 標準輸出, 標準錯誤)
            }
        }
        Err(e) => format!("無法啟動 Podman 沙箱 (請確認 Podman Machine 已啟動): {}", e),
    }
}
