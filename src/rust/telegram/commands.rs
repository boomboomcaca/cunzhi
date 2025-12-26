use crate::config::{save_config, AppState, TelegramConfig};
use crate::constants::telegram as telegram_constants;
use crate::telegram::{
    handle_callback_query, handle_text_message, CallbackQueryResult, TelegramCore,
};
use crate::log_important;
use tauri::{AppHandle, Emitter, Manager, State};
use teloxide::prelude::*;

/// 获取Telegram配置
#[tauri::command]
pub async fn get_telegram_config(state: State<'_, AppState>) -> Result<TelegramConfig, String> {
    let config = state
        .config
        .lock()
        .map_err(|e| format!("获取配置失败: {}", e))?;
    Ok(config.telegram_config.clone())
}

/// 设置Telegram配置
#[tauri::command]
pub async fn set_telegram_config(
    telegram_config: TelegramConfig,
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<(), String> {
    {
        let mut config = state
            .config
            .lock()
            .map_err(|e| format!("获取配置失败: {}", e))?;
        config.telegram_config = telegram_config;
    }

    // 保存配置到文件
    save_config(&state, &app)
        .await
        .map_err(|e| format!("保存配置失败: {}", e))?;

    Ok(())
}

/// 测试Telegram Bot连接
#[tauri::command]
pub async fn test_telegram_connection_cmd(
    bot_token: String,
    chat_id: String,
    state: State<'_, AppState>,
) -> Result<String, String> {
    // 获取API URL配置
    let api_url = {
        let config = state
            .config
            .lock()
            .map_err(|e| format!("获取配置失败: {}", e))?;
        config.telegram_config.api_base_url.clone()
    };

    // 使用默认API URL时传递None，否则传递自定义URL
    let api_url_option = if api_url == telegram_constants::API_BASE_URL {
        None
    } else {
        Some(api_url.as_str())
    };

    crate::telegram::core::test_telegram_connection_with_api_url(&bot_token, &chat_id, api_url_option)
        .await
        .map_err(|e| e.to_string())
}

/// 自动获取Chat ID（通过监听Bot消息）
#[tauri::command]
pub async fn auto_get_chat_id(
    bot_token: String,
    app_handle: AppHandle,
) -> Result<(), String> {
    // 获取API URL配置
    let mut bot = Bot::new(bot_token.clone());
    
    if let Some(state) = app_handle.try_state::<AppState>() {
        if let Ok(config) = state.config.lock() {
            let api_url = &config.telegram_config.api_base_url;
            if api_url != telegram_constants::API_BASE_URL {
                if let Ok(url) = reqwest::Url::parse(api_url) {
                    bot = bot.set_api_url(url);
                }
            }
        }
    }

    // 发送事件通知前端开始监听
    if let Err(e) = app_handle.emit("chat-id-detection-started", ()) {
        log_important!(warn, "发送Chat ID检测开始事件失败: {}", e);
    }

    // 启动临时监听器来获取Chat ID
    let app_handle_clone = app_handle.clone();
    tokio::spawn(async move {
        let mut timeout_count = 0;
        const MAX_TIMEOUT_COUNT: u32 = 30; // 30秒超时

        loop {
            match bot.get_updates().send().await {
                Ok(updates) => {
                    for update in updates {
                        if let teloxide::types::UpdateKind::Message(message) = update.kind {
                            let chat_id = message.chat.id.0.to_string();
                            let chat_title = message.chat.title().unwrap_or("私聊").to_string();
                            let username = message.from.as_ref()
                                .and_then(|u| u.username.as_ref())
                                .map(|s| s.as_str())
                                .unwrap_or("未知用户");

                            // 发送检测到的Chat ID到前端
                            let chat_info = serde_json::json!({
                                "chat_id": chat_id,
                                "chat_title": chat_title,
                                "username": username,
                                "message_text": message.text().unwrap_or(""),
                            });

                            if let Err(e) = app_handle_clone.emit("chat-id-detected", chat_info) {
                                log_important!(warn, "发送Chat ID检测事件失败: {}", e);
                            }

                            return; // 检测到第一个消息后退出
                        }
                    }
                }
                Err(e) => {
                    log_important!(warn, "获取Telegram更新失败: {}", e);
                }
            }

            // 超时检查
            timeout_count += 1;
            if timeout_count >= MAX_TIMEOUT_COUNT {
                if let Err(e) = app_handle_clone.emit("chat-id-detection-timeout", ()) {
                    log_important!(warn, "发送Chat ID检测超时事件失败: {}", e);
                }
                break;
            }

            tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
        }
    });

    Ok(())
}

/// 发送Telegram消息（供其他模块调用）
pub async fn send_telegram_message(
    bot_token: &str,
    chat_id: &str,
    message: &str,
) -> Result<(), String> {
    send_telegram_message_with_markdown(bot_token, chat_id, message, false).await
}

/// 发送支持Markdown的Telegram消息
pub async fn send_telegram_message_with_markdown(
    bot_token: &str,
    chat_id: &str,
    message: &str,
    use_markdown: bool,
) -> Result<(), String> {
    let core =
        TelegramCore::new(bot_token.to_string(), chat_id.to_string()).map_err(|e| e.to_string())?;

    core.send_message_with_markdown(message, use_markdown)
        .await
        .map_err(|e| e.to_string())
}

/// 启动Telegram同步（完整版本）
#[tauri::command]
pub async fn start_telegram_sync(
    message: String,
    predefined_options: Vec<String>,
    is_markdown: bool,
    state: State<'_, AppState>,
    app_handle: AppHandle,
) -> Result<(), String> {
    // 获取Telegram配置
    let (enabled, bot_token, chat_id, continue_reply_enabled) = {
        let config = state
            .config
            .lock()
            .map_err(|e| format!("获取配置失败: {}", e))?;
        (
            config.telegram_config.enabled,
            config.telegram_config.bot_token.clone(),
            config.telegram_config.chat_id.clone(),
            config.reply_config.enable_continue_reply,
        )
    };

    if !enabled {
        return Ok(());
    }

    if bot_token.trim().is_empty() || chat_id.trim().is_empty() {
        return Err("Telegram配置不完整".to_string());
    }

    // 获取API URL配置
    let api_url = {
        let config = state
            .config
            .lock()
            .map_err(|e| format!("获取配置失败: {}", e))?;
        config.telegram_config.api_base_url.clone()
    };

    // 使用默认API URL时传递None，否则传递自定义URL
    let api_url_option = if api_url == telegram_constants::API_BASE_URL {
        None
    } else {
        Some(api_url)
    };

    // 创建Telegram核心实例
    let core = TelegramCore::new_with_api_url(bot_token.clone(), chat_id.clone(), api_url_option)
        .map_err(|e| format!("创建Telegram核心失败: {}", e))?;

    // 发送选项消息
    core.send_options_message(&message, &predefined_options, is_markdown)
        .await
        .map_err(|e| format!("发送选项消息失败: {}", e))?;

    // 短暂延迟确保消息顺序
    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

    // 发送操作消息
    core.send_operation_message(continue_reply_enabled)
        .await
        .map_err(|e| format!("发送操作消息失败: {}", e))?;

    // 启动消息监听（根据是否有预定义选项选择监听模式）
    let bot_token_clone = bot_token.clone();
    let chat_id_clone = chat_id.clone();
    let app_handle_clone = app_handle.clone();

    tokio::spawn(async move {
        // 使用统一的监听器，传递选项参数
        match start_telegram_listener(
            bot_token_clone,
            chat_id_clone,
            app_handle_clone,
            predefined_options,
        )
        .await
        {
            Ok(_) => {}
            Err(e) => log_important!(warn, "Telegram消息监听出错: {}", e),
        }
    });

    Ok(())
}

/// 启动Telegram消息监听（统一版本，支持有选项和无选项模式）
async fn start_telegram_listener(
    bot_token: String,
    chat_id: String,
    app_handle: AppHandle,
    predefined_options_list: Vec<String>,
) -> Result<(), String> {
    // 从AppHandle获取应用状态来读取API URL配置
    let api_url = match app_handle.try_state::<AppState>() {
        Some(state) => {
            let config = state
                .config
                .lock()
                .map_err(|e| format!("获取配置失败: {}", e))?;
            let api_url = config.telegram_config.api_base_url.clone();
                         if api_url == telegram_constants::API_BASE_URL {
                None
            } else {
                Some(api_url)
            }
        }
        None => None, // 如果无法获取状态，使用默认API
    };

    let core = TelegramCore::new_with_api_url(bot_token, chat_id, api_url)
        .map_err(|e| format!("创建Telegram核心失败: {}", e))?;

    let mut offset = 0i32;

    // 用于跟踪选项状态和消息ID
    let mut selected_options: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut options_message_id: Option<i32> = None;
    let mut user_input: String = String::new(); // 存储用户输入的文本
    let predefined_options = predefined_options_list;
    let has_options = !predefined_options.is_empty(); // 是否有预定义选项

    // 获取当前最新的消息ID作为基准
    if let Ok(updates) = core.bot.get_updates().limit(10).await {
        if let Some(update) = updates.last() {
            offset = update.id.0 as i32 + 1;
        }
    }

    // 监听循环
    loop {
        match core.bot.get_updates().offset(offset).timeout(10).await {
            Ok(updates) => {
                for update in updates {
                    offset = update.id.0 as i32 + 1;

                    match update.kind {
                        teloxide::types::UpdateKind::CallbackQuery(callback_query) => {
                            // 今callback_query中提取消息ID
                            if let Some(message) = &callback_query.message {
                                if options_message_id.is_none() && has_options {
                                    options_message_id = Some(message.id().0);
                                }
                            }

                            if let Ok(Some(result)) =
                                handle_callback_query(&core.bot, &callback_query, core.chat_id)
                                    .await
                            {
                                use crate::telegram::TelegramEvent;
                                
                                match result {
                                    CallbackQueryResult::OptionToggled(option) => {
                                        // 只有当有预定义选项时才处理选项切换
                                        if has_options {
                                            // 切换选项状态
                                            let selected = if selected_options.contains(&option) {
                                                selected_options.remove(&option);
                                                false
                                            } else {
                                                selected_options.insert(option.clone());
                                                true
                                            };

                                            // 发送事件到前端
                                            let event = TelegramEvent::OptionToggled {
                                                option: option.clone(),
                                                selected,
                                            };
                                            let _ = app_handle.emit("telegram-event", &event);

                                            // 更新按钮状态
                                            if let Some(msg_id) = options_message_id {
                                                let selected_vec: Vec<String> =
                                                    selected_options.iter().cloned().collect();
                                                let _ = core
                                                    .update_inline_keyboard(
                                                        msg_id,
                                                        &predefined_options,
                                                        &selected_vec,
                                                    )
                                                    .await;
                                            }
                                        }
                                    }
                                    CallbackQueryResult::EnhancePressed => {
                                        // 增强按钮点击 - 参照发送和继续按钮的实现
                                        // 构建增强 prompt
                                        let enhance_prompt = format!(
                                            r#"Use the following prompt to optimize and enhance the context of the content in 《》, and return the enhanced result by calling the tool '寸止' after completion.Here is an instruction that I'd like to give you, but it needs to be improved. Rewrite and enhance this instruction to make it clearer, more specific, less ambiguous, and correct any mistakes. Reply immediately with your answer, even if you're not sure. Consider the context of our conversation history when enhancing the prompt. Reply with the following format:

### BEGIN RESPONSE ###
Here is an enhanced version of the original instruction that is more specific and clear:
<augment-enhanced-prompt>enhanced prompt goes here</augment-enhanced-prompt>

### END RESPONSE ###

Here is my original instruction:

《{}》"#,
                                            user_input
                                        );
                                        
                                        // 发送确认消息
                                        let _ = core.send_message(&format!("✨ 增强请求已发送\n\n📝 原文：{}", user_input)).await;
                                        
                                        // 发送增强事件到前端，携带构建好的 prompt
                                        let _ = app_handle.emit("telegram-event", &TelegramEvent::EnhancePressed { text: enhance_prompt });
                                    }
                                    CallbackQueryResult::ContinuePressed => {
                                        // 继续按钮点击
                                        let feedback_message =
                                            crate::telegram::core::build_feedback_message(
                                                &[],
                                                "",
                                                true,
                                            );
                                        let _ = core.send_message(&feedback_message).await;
                                        let _ = app_handle.emit("telegram-event", &TelegramEvent::ContinuePressed);
                                    }
                                    CallbackQueryResult::SendPressed => {
                                        // 发送按钮点击
                                        let selected_list: Vec<String> =
                                            selected_options.iter().cloned().collect();
                                        let feedback_message =
                                            crate::telegram::core::build_feedback_message(
                                                &selected_list,
                                                &user_input,
                                                false,
                                            );
                                        let _ = core.send_message(&feedback_message).await;
                                        let _ = app_handle.emit("telegram-event", &TelegramEvent::SendPressed);
                                    }
                                }
                            }
                        }
                        teloxide::types::UpdateKind::Message(message) => {
                            // 只有当有预定义选项时才检查 inline keyboard
                            if has_options {
                                // 检查是否是包含 inline keyboard 的选项消息
                                if let Some(inline_keyboard) = message.reply_markup() {
                                    // 检查是否包含我们的选项按钮
                                    let mut contains_our_options = false;
                                    for row in &inline_keyboard.inline_keyboard {
                                        for button in row {
                                            if let teloxide::types::InlineKeyboardButtonKind::CallbackData(callback_data) = &button.kind {
                                                if callback_data.starts_with("toggle:") {
                                                    contains_our_options = true;
                                                    break;
                                                }
                                            }
                                        }
                                        if contains_our_options {
                                            break;
                                        }
                                    }

                                    if contains_our_options {
                                        options_message_id = Some(message.id.0);
                                    }
                                }
                            }

                            if let Ok(Some(event)) = handle_text_message(
                                &message,
                                core.chat_id,
                                None, // 简化版本不过滤消息ID
                            )
                            .await
                            {
                                // 处理发送和继续按钮，发送反馈消息
                                match &event {
                                    crate::telegram::TelegramEvent::SendPressed => {
                                        let selected_list: Vec<String> =
                                            selected_options.iter().cloned().collect();

                                        // 使用统一的反馈消息生成函数
                                        let feedback_message =
                                            crate::telegram::core::build_feedback_message(
                                                &selected_list,
                                                &user_input,
                                                false, // 不是继续操作
                                            );

                                        let _ = core.send_message(&feedback_message).await;
                                    }
                                    crate::telegram::TelegramEvent::ContinuePressed => {
                                        // 使用统一的反馈消息生成函数
                                        let feedback_message =
                                            crate::telegram::core::build_feedback_message(
                                                &[],  // 继续操作没有选项
                                                "",   // 继续操作没有用户输入
                                                true, // 是继续操作
                                            );

                                        let _ = core.send_message(&feedback_message).await;
                                    }
                                    crate::telegram::TelegramEvent::TextUpdated { text } => {
                                        // 保存用户输入的文本
                                        user_input = text.clone();
                                    }
                                    _ => {
                                        // 其他事件不需要发送反馈消息
                                    }
                                }

                                let _ = app_handle.emit("telegram-event", &event);
                            }
                        }
                        _ => {
                            // 忽略其他类型的更新
                        }
                    }
                }
            }
            Err(_) => {
                tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
            }
        }

        // 短暂延迟避免过于频繁的请求
        tokio::time::sleep(tokio::time::Duration::from_millis(1000)).await;
    }
}
