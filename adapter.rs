use crate::{對話請求, 結局};
use reqwest::Client;
use serde_json::{json, Value};
use std::env;
use std::sync::OnceLock;
use std::time::Duration;

// 設定讀取（config.json + 環境變數）
fn 全域設定() -> &'static Value {
    static 設定: OnceLock<Value> = OnceLock::new();
    設定.get_or_init(|| {
        std::fs::read_to_string("config.json")
            .ok()
            .and_then(|文字| serde_json::from_str(&文字).ok())
            .unwrap_or(json!({}))
    })
}

fn 設定路徑(節點: &str, 鍵: &str) -> String {
    if 節點.is_empty() {
        format!("/{}", 鍵)
    } else {
        format!("/{}/{}", 節點, 鍵)
    }
}


pub fn 設定值(節點: &str, 鍵: &str, 環境鍵: &str, 預設: &str) -> String {
    if let Ok(值) = env::var(環境鍵) {
        return 值;
    }
    match 全域設定().pointer(&設定路徑(節點, 鍵)) {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Bool(b)) => b.to_string(),
        Some(Value::Number(n)) => n.to_string(),
        _ => 預設.to_string(),
    }
}

pub fn 設定布林(節點: &str, 鍵: &str, 環境鍵: &str, 預設: bool) -> bool {
    let 預設值 = if 預設 { "1" } else { "0" };
    matches!(
        設定值(節點, 鍵, 環境鍵, 預設值)
            .to_ascii_lowercase()
            .as_str(),
        "1" | "true" | "yes" | "on"
    )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum 後端種類 {
    OpenRouter,
    Ollama,
    LMStudio,
}

impl 後端種類 {
    pub fn 解析(名: &str) -> Option<Self> {
        match 名.trim().to_ascii_lowercase().as_str() {
            "openrouter" | "or" | "雲端" => Some(Self::OpenRouter),
            "ollama" | "ol"|"羊駝" => Some(Self::Ollama),
            "lmstudio" | "lm-studio" | "lms" | "lm" => Some(Self::LMStudio),
            _ => None,
        }
    }

    pub fn 選單() -> &'static str {
        "openrouter | ollama | lmstudio"
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum 工具策略 {
    開啟,
    關閉,
    自動,
}

impl 工具策略 {
    pub fn 解析(值: &str) -> Option<Self> {
        match 值.trim().to_ascii_lowercase().as_str() {
            "on" | "1" | "true" | "yes" | "開" | "啟" => Some(Self::開啟),
            "off" | "0" | "false" | "no" | "關" | "閉" => Some(Self::關閉),
            "auto" | "自動" => Some(Self::自動),
            _ => None,
        }
    }

}

///每個適配器各自持有一份工具設定。
///'探測結果'為 None 代表尚未探測（或探測失敗），此時退回 `保守值`。
#[derive(Clone, Debug)]
pub struct 工具設定 {
    pub 策略: 工具策略,
    pub 探測結果: Option<bool>,
    pub 保守值: bool,
}

impl 工具設定 {
    pub fn 新建(策略: 工具策略, 保守值: bool) -> Self {
        Self {
            策略,
            探測結果: None,
            保守值,
        }
    }

    pub fn 送出(&self) -> bool {
        match self.策略 {
            工具策略::開啟 => true,
            工具策略::關閉 => false,
            工具策略::自動 => self.探測結果.unwrap_or(self.保守值),
        }
    }

    pub fn 狀態(&self) -> String {
        let 結論 = if self.送出() { "會送 tools" } else { "不送 tools" };
        match self.策略 {
            工具策略::開啟 => format!("on（強制 {}）", 結論),
            工具策略::關閉 => format!("off（強制 {}）", 結論),
            工具策略::自動 => match self.探測結果 {
                Some(支援) => format!(
                    "auto（探測：{} → {}）",
                    if 支援 { "模型支援 tools" } else { "模型不支援 tools" },
                    結論
                ),
                None => format!("auto（尚未探測 → 保守 {}）", 結論),
            },
        }
    }
}
//作統一介面用
pub trait 後端適配器 {
    /// 顯示名稱
    fn 名稱(&self) -> &'static str;
    /// 線路格式說明
    fn 格式(&self) -> &'static str;
    /// 完整的 chat completions 端點
    fn 端點(&self) -> &str;
    /// 目前使用的模型識別字
    fn 模型(&self) -> &str;
    /// 執行期覆寫模型（會使能力探測結果失效）
    fn 改模型(&mut self, 模型: String);
    /// HTTP 標頭（含授權）
    fn 標頭(&self) -> 結局<Vec<(&'static str, String)>>;

    // ---- 工具能力 ----
    fn 工具設定(&self) -> &工具設定;
    fn 工具設定可變(&mut self) -> &mut 工具設定;
    /// (探測用 URL, POST body)；None 代表此後端沒有能力查詢 API
    fn 能力探測(&self) -> Option<(String, Value)>;

    fn 送出工具(&self) -> bool {
        self.工具設定().送出()
    }
    fn 設工具策略(&mut self, 策略: 工具策略) {
        self.工具設定可變().策略 = 策略;
    }

    /// 統一請求 → 該後端的線路格式；預設即標準 OpenAI 格式
    fn 編碼請求(&self, 請求: &對話請求<'_>) -> 結局<Vec<u8>> {
        Ok(serde_json::to_vec(請求)?)
    }

    /// 該後端回覆 → 統一格式；預設直通（零拷貝快徑）
    fn 正規化響應(&self, 原始: Vec<u8>) -> 結局<Vec<u8>> {
        Ok(原始)
    }
}

fn 修剪尾斜線(網址: &str) -> String {
    網址.trim_end_matches('/').to_string()
}

fn 解析能力(回覆: &Value) -> bool {
    回覆
        .get("capabilities")
        .and_then(Value::as_array)
        .map(|組| 組.iter().any(|項目| 項目.as_str() == Some("tools")))
        .unwrap_or(false)
}

/// 失敗時保持 None（用保守值），可用 `重探工具能力()` 重試。
pub async fn 預備工具能力(用戶: &Client, 適配器: &mut dyn 後端適配器) {
    if 適配器.工具設定().策略 != 工具策略::自動
        || 適配器.工具設定().探測結果.is_some()
    {
        return;
    }
    let Some((網址, 請求體)) = 適配器.能力探測() else {
        return;
    };

    let 結果: 結局<bool> = async {
        let 回 = 用戶.post(&網址).json(&請求體).send().await?;
        if !回.status().is_success() {
            return Err(format!("HTTP {}", 回.status()).into());
        }
        let 內容: Value = 回.json().await?;
        Ok(解析能力(&內容))
    }
    .await;

    match 結果 {
        Ok(支援) => 適配器.工具設定可變().探測結果 = Some(支援),
        Err(e) => eprintln!(
            "[警告] 無法探測 {} 的工具能力（{}）；改用保守值，可輸入「工具 重探」重試",
            適配器.名稱(),
            e
        ),
    }
}

/// 換模型或後端連線恢復後重新探測
pub async fn 重探工具能力(用戶: &Client, 適配器: &mut dyn 後端適配器) {
    適配器.工具設定可變().探測結果 = None;
    預備工具能力(用戶, 適配器).await;
}

/// 關掉工具時，把請求裡所有工具痕跡清乾淨：
/// 移除 tools 欄位、role=tool 的歷史訊息，以及殘留的 tool_calls / tool_call_id / tool_name。
/// 沒有任何東西需要移除時，交回原始位元組（不重新序列化）。
fn 濾為純文字請求(體: Vec<u8>) -> 結局<Vec<u8>> {
    let mut 根: Value = serde_json::from_slice(&體)?;
    let mut 已變動 = 根
        .as_object_mut()
        .and_then(|物件| 物件.remove("tools"))
        .is_some();

    if let Some(訊息組) = 根.get_mut("messages").and_then(Value::as_array_mut) {
        let 原訊息 = std::mem::take(訊息組);
        for mut 訊息 in 原訊息 {
            if 訊息.get("role").and_then(Value::as_str) == Some("tool") {
                已變動 = true;
                continue;
            }
            if let Some(物件) = 訊息.as_object_mut() {
                for 鍵 in ["tool_calls", "tool_call_id", "tool_name"] {
                    if 物件.remove(鍵).is_some() {
                        已變動 = true;
                    }
                }
            }
            訊息組.push(訊息);
        }
    }

    if 已變動 {
        Ok(serde_json::to_vec(&根)?)
    } else {
        Ok(體)
    }
}

/// 多模態 content 陣列 → 純文字（OpenRouter 部分模型會回 [{type:"text",text:...}]）
fn 取出文字(片段組: &Value) -> String {
    片段組
        .as_array()
        .map(|組| {
            組.iter().fold(String::new(), |mut 累積, 片段| {
                let 型別 = 片段.get("type").and_then(Value::as_str).unwrap_or("text");
                if 型別 == "text" {
                    if let Some(文字) = 片段.get("text").and_then(Value::as_str) {
                        累積.push_str(文字);
                    }
                }
                累積
            })
        })
        .unwrap_or_default()
}

/// 把單一 message 正規化成 main.rs 的 `Message` 可讀的形狀。
/// 回傳 (訊息, 是否變動)；未變動時呼叫端可直接沿用原始 bytes。
fn 正規化訊息(mut 訊息: Value) -> (Value, bool) {
    if !訊息.is_object() {
        return (訊息, false);
    }
    let mut 變動 = false;

    // content: 陣列 → 字串
    if let Some(內容) = 訊息.get("content").cloned() {
        if 內容.is_array() {
            訊息["content"] = Value::String(取出文字(&內容));
            變動 = true;
        }
    }

    // tool_calls: 補 id / type，arguments 物件 → JSON 字串
    if let Some(工具組) = 訊息.get_mut("tool_calls").and_then(Value::as_array_mut) {
        for (序, 工具) in 工具組.iter_mut().enumerate() {
            if !工具.is_object() {
                continue;
            }
            if 工具.get("id").map(Value::is_null).unwrap_or(true) {
                工具["id"] = Value::String(format!("call_{}", 序));
                變動 = true;
            }
            if 工具.get("type").map(Value::is_null).unwrap_or(true) {
                工具["type"] = Value::String("function".to_string());
                變動 = true;
            }
            if let Some(函式) = 工具.get_mut("function") {
                if 函式.is_object() {
                    if let Some(參數) = 函式.get("arguments").cloned() {
                        if !參數.is_string() && !參數.is_null() {
                            函式["arguments"] = Value::String(參數.to_string());
                            變動 = true;
                        }
                    }
                }
            }
        }
    }

    (訊息, 變動)
}

/// OpenAI 相容格式的共用正規化（OpenRouter / Ollama v1 / LM Studio 皆走這裡）
fn 正規化相容回覆(原始: Vec<u8>, 名稱: &str) -> 結局<Vec<u8>> {
    let 根: Value = match serde_json::from_slice(&原始) {
        Ok(v) => v,
        // 非 JSON（例如反向代理的 HTML 錯誤頁）交給上層印出原始內容更好偵錯
        Err(_) => return Ok(原始),
    };

    if let Some(錯) = 根.get("error") {
        return Err(format!("[{}] 後端回傳錯誤：{}", 名稱, 錯).into());
    }

    let mut 根 = 根;
    let mut 已變動 = false;
    if let Some(選項組) = 根.get_mut("choices").and_then(Value::as_array_mut) {
        for 選項 in 選項組.iter_mut() {
            let Some(訊息) = 選項.get_mut("message") else {
                continue;
            };
            let (新訊息, 變動) = 正規化訊息(訊息.clone());
            if 變動 {
                *訊息 = 新訊息;
                已變動 = true;
            }
        }
    }

    // 關鍵：沒有差異就把原始位元組還回去，後續依舊是零拷貝借用
    if 已變動 {
        Ok(serde_json::to_vec(&根)?)
    } else {
        Ok(原始)
    }
}

// ==================================================================
// 格式一：OpenRouter（雲端 · OpenAI 相容 + Bearer + 來源標頭）
// ==================================================================

pub struct OpenRouter {
    端點: String,
    模型: String,
    金鑰: String,
    工具: 工具設定,
}

impl 後端適配器 for OpenRouter {
    fn 名稱(&self) -> &'static str {
        "OpenRouter"
    }
    fn 格式(&self) -> &'static str {
        "OpenAI 相容 · 雲端 Bearer + 來源標頭"
    }
    fn 端點(&self) -> &str {
        &self.端點
    }
    fn 模型(&self) -> &str {
        &self.模型
    }
    fn 改模型(&mut self, 模型: String) {
        self.模型 = 模型;
        self.工具.探測結果 = None;
    }
    fn 標頭(&self) -> 結局<Vec<(&'static str, String)>> {
        if self.金鑰.is_empty() {
            return Err("OpenRouter 需要在 config.json 填入 openrouter.api_key，或設 OPENROUTER_API_KEY".into());
        }
        Ok(vec![
            ("Content-Type", "application/json".to_string()),
            ("Authorization", format!("Bearer {}", self.金鑰)),
            (
                "HTTP-Referer",
                設定值("openrouter", "referer", "OPENROUTER_REFERER", "https://localhost/coding_tools"),
            ),
            (
                "X-Title",
                設定值("openrouter", "title", "OPENROUTER_TITLE", "coding_tools"),
            ),
        ])
    }
    fn 工具設定(&self) -> &工具設定 {
        &self.工具
    }
    fn 工具設定可變(&mut self) -> &mut 工具設定 {
        &mut self.工具
    }
    fn 能力探測(&self) -> Option<(String, Value)> {
        // OpenRouter 沒有輕量的單模型能力查詢；用保守值 true，必要時手動 off
        None
    }
    fn 編碼請求(&self, 請求: &對話請求<'_>) -> 結局<Vec<u8>> {
        let mut 體: Value = serde_json::to_value(請求)?;
        if let Some(供應商) = 全域設定()
            .pointer("/openrouter/provider")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| env::var("OPENROUTER_PROVIDER").ok())
        {
            體["provider"] = json!({ "order": [供應商], "allow_fallbacks": true });
        }
        Ok(serde_json::to_vec(&體)?)
    }
    fn 正規化響應(&self, 原始: Vec<u8>) -> 結局<Vec<u8>> {
        正規化相容回覆(原始, self.名稱())
    }
}

// ==================================================================
// 格式二：Ollama（本地 · /v1 相容 或 /api/chat 原生）
// ==================================================================

pub struct Ollama {
    主機: String,
    端點: String,
    模型: String,
    原生: bool,
    工具: 工具設定,
}

impl 後端適配器 for Ollama {
    fn 名稱(&self) -> &'static str {
        "Ollama (本地)"
    }
    fn 格式(&self) -> &'static str {
        if self.原生 {
            "Ollama 原生 /api/chat"
        } else {
            "OpenAI 相容 /v1/chat/completions"
        }
    }
    fn 端點(&self) -> &str {
        &self.端點
    }
    fn 模型(&self) -> &str {
        &self.模型
    }
    fn 改模型(&mut self, 模型: String) {
        self.模型 = 模型;
        self.工具.探測結果 = None;
    }
    fn 標頭(&self) -> 結局<Vec<(&'static str, String)>> {
        Ok(vec![
            ("Content-Type", "application/json".to_string()),
            ("Authorization", "Bearer ollama".to_string()),
        ])
    }
    fn 工具設定(&self) -> &工具設定 {
        &self.工具
    }
    fn 工具設定可變(&mut self) -> &mut 工具設定 {
        &mut self.工具
    }
    fn 能力探測(&self) -> Option<(String, Value)> {
        // /api/show 回覆含 capabilities: ["completion", "tools", ...]
        Some((
            format!("{}/api/show", self.主機),
            json!({ "model": self.模型 }),
        ))
    }
    fn 編碼請求(&self, 請求: &對話請求<'_>) -> 結局<Vec<u8>> {
        if !self.原生 {
            return Ok(serde_json::to_vec(請求)?);
        }

        // /api/chat 與 /v1 的差異：預設串流、工具回覆用 tool_name 而非 tool_call_id
        let mut 體: Value = serde_json::to_value(請求)?;
        體["stream"] = json!(false);
        if let Some(訊息組) = 體.get_mut("messages").and_then(Value::as_array_mut) {
            for 訊息 in 訊息組.iter_mut() {
                if 訊息.get("role").and_then(Value::as_str) == Some("tool") {
                    let 工具名 = 訊息
                        .get("tool_call_id")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    if let Some(物件) = 訊息.as_object_mut() {
                        物件.remove("tool_call_id");
                    }
                    訊息["tool_name"] = Value::String(工具名);
                }
            }
        }
        Ok(serde_json::to_vec(&體)?)
    }
    fn 正規化響應(&self, 原始: Vec<u8>) -> 結局<Vec<u8>> {
        if !self.原生 {
            return 正規化相容回覆(原始, self.名稱());
        }

        // Ollama 原生：{ message: {...}, done: true } → { choices: [{ message: {...} }] }
        let 根: Value = serde_json::from_slice(&原始)?;
        if let Some(錯) = 根.get("error") {
            return Err(format!("[Ollama] {}", 錯).into());
        }
        let 原訊息 = 根
            .get("message")
            .cloned()
            .unwrap_or(json!({ "role": "assistant", "content": "" }));
        let (mut 訊息, _) = 正規化訊息(原訊息);

        // 原生格式不提供 tool_call id，改用函式名當 id，讓上面的 tool_name 能對回去
        if let Some(工具組) = 訊息.get_mut("tool_calls").and_then(Value::as_array_mut) {
            for 工具 in 工具組.iter_mut() {
                let 名 = 工具
                    .pointer("/function/name")
                    .and_then(Value::as_str)
                    .unwrap_or("tool")
                    .to_string();
                工具["id"] = Value::String(名);
            }
        }

        Ok(serde_json::to_vec(&json!({
            "choices": [{ "message": 訊息 }]
        }))?)
    }
}

// ==================================================================
// 格式三：LM Studio（本地 · OpenAI 相容、免金鑰）
// ==================================================================

pub struct LMStudio {
    端點: String,
    模型: String,
    工具: 工具設定,
}

impl 後端適配器 for LMStudio {
    fn 名稱(&self) -> &'static str {
        "LM Studio (本地)"
    }
    fn 格式(&self) -> &'static str {
        "OpenAI 相容 · 本地免金鑰"
    }
    fn 端點(&self) -> &str {
        &self.端點
    }
    fn 模型(&self) -> &str {
        &self.模型
    }
    fn 改模型(&mut self, 模型: String) {
        self.模型 = 模型;
        self.工具.探測結果 = None;
    }
    fn 標頭(&self) -> 結局<Vec<(&'static str, String)>> {
        Ok(vec![
            ("Content-Type", "application/json".to_string()),
            ("Authorization", "Bearer lm-studio".to_string()),
        ])
    }
    fn 工具設定(&self) -> &工具設定 {
        &self.工具
    }
    fn 工具設定可變(&mut self) -> &mut 工具設定 {
        &mut self.工具
    }
    fn 能力探測(&self) -> Option<(String, Value)> {
        // LM Studio 的 /v1/models 只回模型 id，不含能力標記；用保守值 + 手動開關
        None
    }
    fn 正規化響應(&self, 原始: Vec<u8>) -> 結局<Vec<u8>> {
        正規化相容回覆(原始, self.名稱())
    }
}

// ==================================================================
// 工廠 + 共用傳輸層
// ==================================================================

/// 優先順序：後端專屬環境變數 > 全域環境變數 AI_TOOLS
///          > config.json 後端欄位 > config.json 全域欄位 > auto
fn 讀取策略(節點: &str, 環境鍵: &str) -> 工具策略 {
    let 設定檔值 = 全域設定()
        .pointer(&設定路徑(節點, "tools"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| {
            全域設定()
                .pointer("/tools")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_else(|| "auto".to_string());

    let 值 = env::var(環境鍵)
        .or_else(|_| env::var("AI_TOOLS"))
        .unwrap_or(設定檔值);

    工具策略::解析(&值).unwrap_or_else(|| {
        eprintln!("[警告] 無效的工具策略 '{}'，改用 auto", 值);
        工具策略::自動
    })
}

pub fn 建立適配器(種類: 後端種類) -> 結局<Box<dyn 後端適配器>> {
    match 種類 {
        後端種類::OpenRouter => {
            let 基底 = 修剪尾斜線(&設定值(
                "openrouter",
                "base_url",
                "OPENROUTER_BASE_URL",
                "https://openrouter.ai/api/v1",
            ));
            let 金鑰 = 設定值("openrouter", "api_key", "OPENROUTER_API_KEY", "");
            Ok(Box::new(OpenRouter {
                端點: format!("{}/chat/completions", 基底),
                模型: 設定值("openrouter", "model", "OPENROUTER_MODEL", "openai/gpt-4o-mini"),
                金鑰,
                工具: 工具設定::新建(讀取策略("openrouter", "OPENROUTER_TOOLS"), true),
            }))
        }
        後端種類::Ollama => {
            let 主機 = 修剪尾斜線(&設定值("ollama", "host", "OLLAMA_HOST", "http://localhost:11434"));
            let 原生 = 設定布林("ollama", "native", "OLLAMA_NATIVE", false);
            let 端點 = if 原生 {
                format!("{}/api/chat", 主機)
            } else {
                format!("{}/v1/chat/completions", 主機)
            };
            Ok(Box::new(Ollama {
                主機,
                端點,
                模型: 設定值("ollama", "model", "OLLAMA_MODEL", "qwen2.5vl:7b"),
                原生,
                // 探測失敗時保守不送，避免 Ollama 回 400 does not support tools
                工具: 工具設定::新建(讀取策略("ollama", "OLLAMA_TOOLS"), false),
            }))
        }
        後端種類::LMStudio => {
            let 主機 = 修剪尾斜線(&設定值("lmstudio", "host", "LMSTUDIO_HOST", "http://localhost:1234"));
            Ok(Box::new(LMStudio {
                端點: format!("{}/v1/chat/completions", 主機),
                模型: 設定值("lmstudio", "model", "LMSTUDIO_MODEL", "qwen/qwen3-vl-4b"),
                工具: 工具設定::新建(讀取策略("lmstudio", "LMSTUDIO_TOOLS"), false),
            }))
        }
    }
}

/// 三種後端共用的 HTTP 傳輸層；適配器只負責給出端點、標頭與格式翻譯。
pub async fn 寄柬(
    用戶: &Client,
    適配器: &dyn 後端適配器,
    請求: &對話請求<'_>,
) -> 結局<Vec<u8>> {
    let 逾時秒數: u64 = 設定值("", "timeout_secs", "AI_TIMEOUT_SECS", "300")
        .parse()
        .unwrap_or(300);

    // 工具開關在傳輸層統一守門：即使呼叫端傳了 tools，off/auto(false) 也不會送出去
    let 體 = 適配器.編碼請求(請求)?;
    let 體 = if 適配器.送出工具() {
        體
    } else {
        濾為純文字請求(體)?
    };

    let mut 柬 = 用戶
        .post(適配器.端點())
        .timeout(Duration::from_secs(逾時秒數))
        .body(體);

    for (名, 值) in 適配器.標頭()? {
        柬 = 柬.header(名, 值);
    }

    let 善 = 柬.send().await?;
    let 狀態 = 善.status();
    // 直接讀取原始位元組流，不做任何 JSON 字串轉換
    let 原始 = Vec::from(善.bytes().await?);

    if !狀態.is_success() {
        return Err(format!(
            "[{}] HTTP {}：{}",
            適配器.名稱(),
            狀態,
            String::from_utf8_lossy(&原始)
        )
        .into());
    }

    適配器.正規化響應(原始)
}

// ==================================================================
// 格式與工具策略驗證
// ==================================================================

#[cfg(test)]
mod 測試 {
    use super::*;
    use crate::Message;
    use std::borrow::Cow;

    fn 建原生適配器() -> Ollama {
        Ollama {
            主機: "http://localhost:11434".to_string(),
            端點: "http://localhost:11434/api/chat".to_string(),
            模型: "qwen2.5vl:7b".to_string(),
            原生: true,
            工具: 工具設定::新建(工具策略::關閉, false),
        }
    }

    #[test]
    fn 三種後端端點與格式各不相同() {
        let 清單 = [
            (後端種類::OpenRouter, "https://openrouter.ai/api/v1/chat/completions"),
            (後端種類::Ollama, "http://localhost:11434/v1/chat/completions"),
            (後端種類::LMStudio, "http://localhost:1234/v1/chat/completions"),
        ];
        for (種類, 預期端點) in 清單 {
            let a = 建立適配器(種類).unwrap();
            assert_eq!(a.端點(), 預期端點);
            assert!(!a.格式().is_empty());
        }
    }

    // ---- 工具能力 ----

    #[test]
    fn 解析ollama真實能力標記() {
        // 實測：兩個模型都沒有 tools
        assert!(!解析能力(&json!({ "capabilities": ["completion", "vision"] })));
        assert!(!解析能力(&json!({ "capabilities": ["thinking", "completion"] })));
        assert!(解析能力(&json!({ "capabilities": ["completion", "tools"] })));
        assert!(!解析能力(&json!({ "model_info": {} })));
    }

    #[test]
    fn 三種策略與探測結果的組合() {
        let mut 設定 = 工具設定::新建(工具策略::自動, false);
        assert!(!設定.送出()); // 未探測 → 保守 false
        設定.探測結果 = Some(true);
        assert!(設定.送出());
        設定.策略 = 工具策略::關閉;
        assert!(!設定.送出()); // off 覆寫探測結果
        設定.策略 = 工具策略::開啟;
        assert!(設定.送出());

        let 雲端 = 工具設定::新建(工具策略::自動, true);
        assert!(雲端.送出()); // OpenRouter 保守 true
    }

    #[test]
    fn ollama探測端點指向api_show並帶目前模型() {
        let a = 建原生適配器();
        let (網址, 體) = a.能力探測().unwrap();
        assert_eq!(網址, "http://localhost:11434/api/show");
        assert_eq!(體["model"], "qwen2.5vl:7b");
    }

    #[test]
    fn 關閉工具時請求完全不含任何工具痕跡() {
        let 體 = serde_json::to_vec(&json!({
            "model": "qwen2.5vl:7b",
            "messages": [
                { "role": "user", "content": "跑個程式" },
                { "role": "assistant", "content": null, "tool_calls": [{"id":"c0","type":"function","function":{"name":"行沙箱代碼","arguments":"{}"}}] },
                { "role": "tool", "content": "執行成功", "tool_call_id": "c0" },
                { "role": "user", "content": "然後呢" }
            ],
            "tools": [{ "type": "function", "function": { "name": "行沙箱代碼" } }]
        }))
        .unwrap();

        let 結果 = 濾為純文字請求(體).unwrap();
        let v: Value = serde_json::from_slice(&結果).unwrap();
        assert!(v.get("tools").is_none());
        assert_eq!(v["messages"].as_array().unwrap().len(), 3); // tool 訊息被移除
        assert!(v["messages"][1].get("tool_calls").is_none());
        assert_eq!(v["messages"][2]["content"], "然後呢");
    }

    #[test]
    fn 純文字請求若已乾淨則原封不動() {
        let 體 = br#"{"model":"m","messages":[{"role":"user","content":"hi"}]}"#.to_vec();
        let 指標 = 體.as_ptr();
        let 結果 = 濾為純文字請求(體).unwrap();
        assert_eq!(結果.as_ptr(), 指標);
    }

    // ---- 原有格式正規化 ----

    #[test]
    fn ollama原生回覆轉成統一choices格式() {
        let 原始 = serde_json::to_vec(&json!({
            "model": "qwen2.5vl:7b",
            "message": {
                "role": "assistant",
                "content": "",
                "tool_calls": [{
                    "function": {
                        "name": "行沙箱代碼",
                        "arguments": { "語言": "python", "代碼": "print(1)" }
                    }
                }]
            },
            "done": true
        }))
        .unwrap();

        let 結果 = 建原生適配器().正規化響應(原始).unwrap();
        let v: Value = serde_json::from_slice(&結果).unwrap();
        let 訊息 = &v["choices"][0]["message"];
        assert_eq!(訊息["tool_calls"][0]["id"], "行沙箱代碼");
        assert_eq!(訊息["tool_calls"][0]["type"], "function");
        let 參數 = 訊息["tool_calls"][0]["function"]["arguments"]
            .as_str()
            .unwrap();
        assert_eq!(serde_json::from_str::<Value>(參數).unwrap()["語言"], "python");
    }

    #[test]
    fn openrouter多模態內容陣列轉純文字() {
        let a = OpenRouter {
            端點: "https://openrouter.ai/api/v1/chat/completions".to_string(),
            模型: "openai/gpt-4o-mini".to_string(),
            金鑰: "test-key".to_string(),
            工具: 工具設定::新建(工具策略::開啟, true),
        };
        let 原始 = serde_json::to_vec(&json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": [
                        { "type": "text", "text": "哈囉" },
                        { "type": "text", "text": "世界" }
                    ]
                }
            }]
        }))
        .unwrap();

        let 結果 = a.正規化響應(原始).unwrap();
        let v: Value = serde_json::from_slice(&結果).unwrap();
        assert_eq!(v["choices"][0]["message"]["content"], "哈囉世界");
    }

    #[test]
    fn 相容格式無差異時傳回原始位元組() {
        let a = LMStudio {
            端點: "http://localhost:1234/v1/chat/completions".to_string(),
            模型: "local-model".to_string(),
            工具: 工具設定::新建(工具策略::關閉, false),
        };
        let 原始 = r#"{"choices":[{"message":{"role":"assistant","content":"安安"}}]}"#
            .as_bytes()
            .to_vec();
        let 指標 = 原始.as_ptr();
        let 結果 = a.正規化響應(原始).unwrap();
        assert_eq!(結果.as_ptr(), 指標);
    }

    #[test]
    fn ollama原生請求關閉串流並改用tool_name() {
        let 訊息集 = vec![
            Message {
                role: Cow::Borrowed("assistant"),
                content: None,
                tool_calls: None,
                tool_call_id: None,
            },
            Message {
                role: Cow::Borrowed("tool"),
                content: Some(Cow::Borrowed("執行成功")),
                tool_calls: None,
                tool_call_id: Some("行沙箱代碼".to_string()),
            },
        ];
        let 請求 = 對話請求 {
            model: Cow::Borrowed("qwen2.5vl:7b"),
            messages: &訊息集,
            tools: None,
        };

        let 體 = 建原生適配器().編碼請求(&請求).unwrap();
        let v: Value = serde_json::from_slice(&體).unwrap();
        assert_eq!(v["stream"], false);
        assert_eq!(v["messages"][1]["tool_name"], "行沙箱代碼");
        assert!(v["messages"][1].get("tool_call_id").is_none());
    }
}
