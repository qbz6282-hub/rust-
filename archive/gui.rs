//! ==================================================================
//! 圖形介面層：同步 `UI` 與非同步後端的橋接
//! ==================================================================
//! egui 是「立即模式」GUI：`update()` 由主執行緒每幀呼叫，本身是同步函式。
//! 若直接在 `update()` 裡 `.await` 或 `block_on()`，等網路時視窗會整個人凍住，
//! 連「載入中」的圓圈都畫不出來。
//!
//! 解法：把「等待」交給另一條執行緒。
//!
//!   [UI 執行緒 - 同步]  --指令(mpsc)-->  [背景工作執行緒 - tokio]
//!   [UI 執行緒 - 同步]  <--回報(mpsc)--  [背景工作執行緒 - tokio]
//!                      <-- request_repaint() --- (有新訊息時叫 UI 醒來)
//!
//! 關鍵設計：適配器、上下文、reqwest庫的Client，皆「在背景執行緒內部」建立，
//! 從來沒有跨執行緒移動，因此：
//!   * `Box<dyn 後端適配器>` 不需要加 `Send` 約束
//!   * `對話請求<'q>` 的借用不必改成 `'static`
//!   * 原本 CLI 的 agent 迴圈幾乎可以原樣重用

use eframe::egui;
use serde_json::{json, Value};
use std::borrow::Cow;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;

use crate::apdater::{self, 後端種類, 後端適配器, 工具策略};
use crate::{結局, 對話請求, 對話響應, Message, run_code, 證據路徑, 讀取後端種類};

const 最大工具步數: usize = 5;

// 兩端溝通的訊息
enum 指令 {
    對話(String),
    切換後端(後端種類),
    改模型(String),
    設工具(工具策略),
    清空,
}

enum 回報 {
    /// 追加一行到聊天記錄
    一行(String),
    /// 更新上方狀態列
    狀態(String),
    /// 是否正在等待模型（控制輸入框與 spinner）
    忙碌(bool),
}

fn 推送(送: &Sender<回報>, 上下文: &egui::Context, 訊息: 回報) {
    let _ = 送.send(訊息);
    // 背景執行緒不能直接改畫面，只能「請求下一幀重繪」
    上下文.request_repaint();
}

fn 狀態文字(適配器: &dyn 後端適配器) -> String {
    format!(
        "{} ｜ {} ｜ 工具：{}",
        適配器.名稱(),
        適配器.模型(),
        適配器.工具設定().狀態()
    )
}

fn 工具名稱(策略: 工具策略) -> &'static str {
    match 策略 {
        工具策略::開啟 => "on",
        工具策略::關閉 => "off",
        工具策略::自動 => "auto",
    }
}

fn 建立利器() -> Vec<Value> {
    vec![json!({
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
    })]
}

/// egui 內建字型不含中文字，直接載入 Windows 系統字型。
/// 回傳實際採用的路徑，方便在介面上顯示。
fn 載入中文字型(ctx: &egui::Context) -> String {
    // 優先選單一 TTF（可變字重檔），TTC 字型集合往後放
    let 候選 = [
        "C:/Windows/Fonts/NotoSansTC-VF.ttf",
        "C:/Windows/Fonts/msjh.ttc",
        "C:/Windows/Fonts/msyh.ttc",
        "C:/Windows/Fonts/mingliu.ttc",
    ];
    for 路徑 in 候選 {
        let Ok(位元組) = std::fs::read(路徑) else {
            continue;
        };
        let mut 定義 = egui::FontDefinitions::default();
        定義.font_data.insert(
            "chinese".to_owned(),
            Arc::new(egui::FontData::from_owned(位元組)),
        );
        if let Some(家族) = 定義.families.get_mut(&egui::FontFamily::Proportional) {
            家族.insert(0, "chinese".to_owned());
        }
        if let Some(家族) = 定義.families.get_mut(&egui::FontFamily::Monospace) {
            家族.push("chinese".to_owned());
        }
        ctx.set_fonts(定義);
        return 路徑.to_string();
    }
    "內建字型（找不到系統中文字型，中文可能顯示為方塊）".to_string()
}

// ===================================
// 背景工作執行緒：所有 async 都活在這裡
// ===================================

fn 啟動背景工作(指令收: Receiver<指令>, 回報送: Sender<回報>, 上下文: egui::Context) {
    std::thread::Builder::new()
        .name("ai-worker".into())
        .spawn(move || {
            let rt = match tokio::runtime::Runtime::new() {
                Ok(r) => r,
                Err(e) => {
                    推送(&回報送, &上下文, 回報::一行(format!("[錯誤] 無法建立 tokio runtime：{}", e)));
                    return;
                }
            };

            let 用戶 = reqwest::Client::new();
            let mut 適配器 = match apdater::建立適配器(讀取後端種類()) {
                Ok(a) => a,
                Err(e) => {
                    推送(&回報送, &上下文, 回報::一行(format!("[錯誤] 無法建立適配器：{}", e)));
                    return;
                }
            };
            let mut 後續上下文: Vec<Message<'static>> = Vec::new();
            let 利器 = 建立利器();
            let 證據路徑 = 證據路徑();

            // 啟動時的能力探測也是 async，但在這裡用 block_on 完全沒問題，
            // 因為這條執行緒本來就專門負責等待，不是畫圖的主執行緒。
            rt.block_on(apdater::預備工具能力(&用戶, &mut *適配器));

            推送(&回報送, &上下文, 回報::狀態(狀態文字(&*適配器)));
            推送(&回報送, &上下文, 回報::一行("[系統] 背景工作執行緒已就緒".into()));

            // recv() 會阻塞等待，不燒 CPU；UI 關掉後通道斷開，執行緒自然結束
            while let Ok(指令) = 指令收.recv() {
                rt.block_on(處理指令(
                    指令,
                    &mut 適配器,
                    &mut 後續上下文,
                    &利器,
                    &用戶,
                    &證據路徑,
                    &回報送,
                    &上下文,
                ));
            }
        })
        .expect("無法啟動背景工作執行緒");
}

/// 一則指令 = 一個完整的 agent 回合（可能含多次工具呼叫）。
/// 邊跑邊送回報，所以工具執行過程也能即時顯示在畫面上。
async fn 處理指令(
    指令: 指令,
    適配器: &mut Box<dyn 後端適配器>,
    上下文: &mut Vec<Message<'static>>,
    利器: &[Value],
    用戶: &reqwest::Client,
    證據路徑: &str,
    回報送: &Sender<回報>,
    ctx: &egui::Context,
) {
    match 指令 {
        指令::對話(文字) => {
            推送(回報送, ctx, 回報::忙碌(true));
            上下文.push(Message::起會("user", 文字));

            for 第幾步 in 0..=最大工具步數 {
                let 請求資訊 = 對話請求 {
                    model: Cow::Borrowed(適配器.模型()),
                    messages: 上下文.as_slice(),
                    tools: Some(利器),
                };

                let 緩衝 = match apdater::寄柬(用戶, &**適配器, &請求資訊).await {
                    Ok(b) => b,
                    Err(e) => {
                        推送(回報送, ctx, 回報::一行(format!("[錯誤] 請求失敗：{}", e)));
                        上下文.pop();
                        break;
                    }
                };

                let 響應: 對話響應 = match serde_json::from_slice(&緩衝) {
                    Ok(r) => r,
                    Err(e) => {
                        推送(
                            回報送,
                            ctx,
                            回報::一行(format!(
                                "[解析錯誤] {}；原始回傳：{}",
                                e,
                                String::from_utf8_lossy(&緩衝)
                            )),
                        );
                        上下文.pop();
                        break;
                    }
                };

                let Some(首選) = 響應.choices.into_iter().next() else {
                    推送(回報送, ctx, 回報::一行("[警告] 模型未回傳任何選項".into()));
                    break;
                };
                let 模型訊息 = 首選.訊息;
                let 允許工具 = 適配器.送出工具();

                if let Some(工具集) = 模型訊息
                    .tool_calls
                    .as_ref()
                    .filter(|組| 允許工具 && !組.is_empty())
                {
                    上下文.push(模型訊息.clone().轉為永久所有權());
                    for 工具 in 工具集 {
                        推送(
                            回報送,
                            ctx,
                            回報::一行(format!("[工具] {}", 工具.function.name)),
                        );
                        let 執行結果 = run_code(&工具.function, 證據路徑).await;
                        推送(
                            回報送,
                            ctx,
                            回報::一行(format!("[沙箱] {}", 執行結果.trim())),
                        );
                        上下文.push(Message {
                            role: Cow::Borrowed("tool"),
                            content: Some(Cow::Owned(執行結果)),
                            tool_calls: None,
                            tool_call_id: Some(工具.id.clone()),
                        });
                    }
                    if 第幾步 == 最大工具步數 {
                        推送(
                            回報送,
                            ctx,
                            回報::一行(format!("[熔斷] 連續工具呼叫超過 {} 次", 最大工具步數)),
                        );
                        break;
                    }
                    continue;
                }

                match &模型訊息.content {
                    Some(內容) if !內容.is_empty() => {
                        推送(回報送, ctx, 回報::一行(format!("模型：{}", 內容)));
                    }
                    _ => 推送(回報送, ctx, 回報::一行("[模型未回傳文字]".into())),
                }
                if 模型訊息.tool_calls.is_some() && !允許工具 {
                    推送(
                        回報送,
                        ctx,
                        回報::一行("[提示] 工具已停用，忽略模型回傳的 tool_calls".into()),
                    );
                }
                上下文.push(模型訊息.轉為永久所有權());
                break;
            }

            推送(回報送, ctx, 回報::忙碌(false));
        }

        指令::切換後端(種類) => match apdater::建立適配器(種類) {
            Ok(新適配器) => {
                *適配器 = 新適配器;
                apdater::預備工具能力(用戶, &mut **適配器).await;
                上下文.clear();
                推送(
                    回報送,
                    ctx,
                    回報::一行(format!("[系統] 已切換至 {}，對話紀錄已清空", 適配器.名稱())),
                );
                推送(回報送, ctx, 回報::狀態(狀態文字(&**適配器)));
            }
            Err(e) => 推送(回報送, ctx, 回報::一行(format!("[錯誤] {}", e))),
        },

        指令::改模型(名稱) => {
            適配器.改模型(名稱);
            apdater::重探工具能力(用戶, &mut **適配器).await;
            推送(
                回報送,
                ctx,
                回報::一行(format!("[系統] 模型已設為 {}", 適配器.模型())),
            );
            推送(回報送, ctx, 回報::狀態(狀態文字(&**適配器)));
        }

        指令::設工具(策略) => {
            適配器.設工具策略(策略);
            apdater::預備工具能力(用戶, &mut **適配器).await;
            推送(回報送, ctx, 回報::狀態(狀態文字(&**適配器)));
        }

        指令::清空 => {
            let 筆數 = 上下文.len();
            上下文.clear();
            推送(
                回報送,
                ctx,
                回報::一行(format!("[系統] 已清空 {} 則對話紀錄", 筆數)),
            );
        }
    }
}

// ==================================================================
// 前景：egui 應用程式
// ==================================================================

pub struct GuiApp {
    輸入框: String,
    聊天記錄: Vec<String>,
    狀態列: String,
    忙碌: bool,

    選定後端: 後端種類,
    選定工具: 工具策略,
    模型輸入: String,

    指令送: Sender<指令>,
    回報收: Receiver<回報>,
}

impl GuiApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let 字型 = 載入中文字型(&cc.egui_ctx);

        let (指令送, 指令收) = mpsc::channel();
        let (回報送, 回報收) = mpsc::channel();
        啟動背景工作(指令收, 回報送, cc.egui_ctx.clone());

        Self {
            輸入框: String::new(),
            聊天記錄: vec![format!("[系統] 中文字型：{}", 字型)],
            狀態列: "正在初始化…".into(),
            忙碌: false,
            選定後端: 讀取後端種類(),
            選定工具: 工具策略::自動,
            模型輸入: String::new(),
            指令送,
            回報收,
        }
    }

    fn 送出(&mut self) {
        let 文字 = self.輸入框.trim().to_string();
        if 文字.is_empty() || self.忙碌 {
            return;
        }
        self.輸入框.clear();
        self.聊天記錄.push(format!("你：{}", 文字));
        self.忙碌 = true;
        let _ = self.指令送.send(指令::對話(文字));
    }

    fn 收取回報(&mut self) {
        // try_recv 不阻塞：沒訊息就立刻回來畫下一幀
        while let Ok(回報) = self.回報收.try_recv() {
            match 回報 {
                回報::一行(文字) => self.聊天記錄.push(文字),
                回報::狀態(文字) => self.狀態列 = 文字,
                回報::忙碌(值) => self.忙碌 = 值,
            }
        }
    }
}

impl eframe::App for GuiApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.收取回報();

        egui::TopBottomPanel::top("設定列").show(ctx, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label("後端");
                let 原本 = self.選定後端;
                egui::ComboBox::from_id_salt("後端選單")
                    .selected_text(match self.選定後端 {
                        後端種類::Ollama => "ollama",
                        後端種類::LMStudio => "lmstudio",
                        後端種類::OpenRouter => "openrouter",
                    })
                    .width(110.0)
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.選定後端, 後端種類::Ollama, "ollama");
                        ui.selectable_value(&mut self.選定後端, 後端種類::LMStudio, "lmstudio");
                        ui.selectable_value(&mut self.選定後端, 後端種類::OpenRouter, "openrouter");
                    });
                if self.選定後端 != 原本 && !self.忙碌 {
                    let _ = self.指令送.send(指令::切換後端(self.選定後端));
                }

                ui.separator();
                ui.label("模型");
                ui.add_enabled(
                    !self.忙碌,
                    egui::TextEdit::singleline(&mut self.模型輸入)
                        .hint_text("輸入模型名稱")
                        .desired_width(180.0),
                );
                if ui
                    .add_enabled(
                        !self.忙碌 && !self.模型輸入.trim().is_empty(),
                        egui::Button::new("套用"),
                    )
                    .clicked()
                {
                    let _ = self
                        .指令送
                        .send(指令::改模型(self.模型輸入.trim().to_string()));
                }

                ui.separator();
                ui.label("工具");
                let 原工具 = self.選定工具;
                egui::ComboBox::from_id_salt("工具選單")
                    .selected_text(工具名稱(self.選定工具))
                    .width(70.0)
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.選定工具, 工具策略::自動, "auto");
                        ui.selectable_value(&mut self.選定工具, 工具策略::開啟, "on");
                        ui.selectable_value(&mut self.選定工具, 工具策略::關閉, "off");
                    });
                if self.選定工具 != 原工具 && !self.忙碌 {
                    let _ = self.指令送.send(指令::設工具(self.選定工具));
                }

                ui.separator();
                if ui.add_enabled(!self.忙碌, egui::Button::new("清空對話")).clicked() {
                    self.聊天記錄.clear();
                    let _ = self.指令送.send(指令::清空);
                }
            });
            ui.label(egui::RichText::new(&self.狀態列).small().weak());
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            egui::ScrollArea::vertical()
                .stick_to_bottom(true)
                .auto_shrink([false; 2])
                .show(ui, |ui| {
                    for 行 in &self.聊天記錄 {
                        ui.label(egui::RichText::new(行));
                        ui.add_space(3.0);
                    }
                });
        });

        egui::TopBottomPanel::bottom("輸入列").show(ctx, |ui| {
            ui.horizontal(|ui| {
                let 輸入框 = ui.add_enabled(
                    !self.忙碌,
                    egui::TextEdit::singleline(&mut self.輸入框)
                        .hint_text("輸入訊息後按 Enter 或點送出")
                        .desired_width(f32::INFINITY),
                );
                let 按了確認鍵 = 輸入框.lost_focus()
                    && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if 按了確認鍵 {
                    self.送出();
                }
                if ui
                    .add_enabled(!self.忙碌, egui::Button::new("送出"))
                    .clicked()
                {
                    self.送出();
                }
            });
            if self.忙碌 {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("模型處理中…（介面仍可操作）");
                });
            }
        });
    }
}

/// 在`main()`呼叫eframe後，會接管主執行緒直至視窗關閉。
pub fn 啟動() -> 結局<()> {
    let 選項 = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([960.0, 680.0])
            .with_min_inner_size([640.0, 420.0]),
        ..Default::default()
    };

    eframe::run_native(
        "coding_tools · 多後端對話",
        選項,
        Box::new(|cc| Ok(Box::new(GuiApp::new(cc)))),
    )
    .map_err(|e| format!("視窗啟動失敗：{}", e))?;
    Ok(())
}
