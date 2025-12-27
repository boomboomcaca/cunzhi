use tauri::{State, Manager};
use crate::config::{AppState, save_config};
use crate::constants::window;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct WindowSizeUpdate {
    pub width: f64,
    pub height: f64,
    pub fixed: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct WindowPositionUpdate {
    pub x: i32,
    pub y: i32,
}

#[tauri::command]
pub async fn apply_window_constraints(state: State<'_, AppState>, app: tauri::AppHandle) -> Result<(), String> {
    let (window_config, always_on_top) = {
        let config = state.config.lock().map_err(|e| format!("获取配置失败: {}", e))?;
        (config.ui_config.window_config.clone(), config.ui_config.always_on_top)
    };

    if let Some(window) = app.get_webview_window("main") {
        // 设置窗口约束
        if let Err(e) = window.set_min_size(Some(tauri::LogicalSize::new(
            window_config.min_width,
            window_config.min_height,
        ))) {
            return Err(format!("设置最小窗口大小失败: {}", e));
        }

        if let Err(e) = window.set_max_size(Some(tauri::LogicalSize::new(
            window_config.max_width,
            window_config.max_height,
        ))) {
            return Err(format!("设置最大窗口大小失败: {}", e));
        }

        // 如果启用了自动调整大小，设置为合适的初始大小
        if window_config.auto_resize {
            let initial_width = window_config.min_width;
            let initial_height = (window_config.min_height + window_config.max_height) / 2.0;
            
            if let Err(e) = window.set_size(tauri::LogicalSize::new(initial_width, initial_height)) {
                return Err(format!("设置窗口大小失败: {}", e));
            }
        }

        // 确保置顶状态在应用窗口约束后仍然有效
        if let Err(e) = window.set_always_on_top(always_on_top) {
            log::warn!("应用窗口约束后重新设置置顶状态失败: {}", e);
        }
    }

    Ok(())
}

#[tauri::command]
pub async fn update_window_size(size_update: WindowSizeUpdate, state: State<'_, AppState>, app: tauri::AppHandle) -> Result<(), String> {
    // 更新配置
    {
        let mut config = state.config.lock().map_err(|e| format!("获取配置失败: {}", e))?;

        // 更新模式设置
        config.ui_config.window_config.fixed = size_update.fixed;

        // 更新当前模式的尺寸
        config.ui_config.window_config.update_current_size(size_update.width, size_update.height);

        if size_update.fixed {
            // 固定模式：设置最大和最小尺寸为相同值
            config.ui_config.window_config.max_width = size_update.width;
            config.ui_config.window_config.max_height = size_update.height;
            config.ui_config.window_config.min_width = size_update.width;
            config.ui_config.window_config.min_height = size_update.height;
            config.ui_config.window_config.auto_resize = false;
        } else {
            // 自由拉伸模式：设置合理的最小值和限制的最大值
            config.ui_config.window_config.min_width = window::MIN_WIDTH;
            config.ui_config.window_config.min_height = window::MIN_HEIGHT;
            config.ui_config.window_config.max_width = window::MAX_WIDTH;
            config.ui_config.window_config.max_height = window::MAX_HEIGHT;
            config.ui_config.window_config.auto_resize = window::DEFAULT_AUTO_RESIZE;
        }
    }

    // 保存配置
    save_config(&state, &app).await.map_err(|e| format!("保存配置失败: {}", e))?;

    // 获取置顶状态
    let always_on_top = {
        let config = state.config.lock().map_err(|e| format!("获取配置失败: {}", e))?;
        config.ui_config.always_on_top
    };

    // 应用到当前窗口
    if let Some(window) = app.get_webview_window("main") {
        if size_update.fixed {
            // 固定模式：设置精确的窗口大小和约束
            if let Err(e) = window.set_size(tauri::LogicalSize::new(size_update.width, size_update.height)) {
                return Err(format!("设置窗口大小失败: {}", e));
            }

            if let Err(e) = window.set_min_size(Some(tauri::LogicalSize::new(size_update.width, size_update.height))) {
                return Err(format!("设置最小窗口大小失败: {}", e));
            }

            if let Err(e) = window.set_max_size(Some(tauri::LogicalSize::new(size_update.width, size_update.height))) {
                return Err(format!("设置最大窗口大小失败: {}", e));
            }

            log::debug!("窗口已设置为固定大小: {}x{}", size_update.width, size_update.height);
        } else {
            // 自由拉伸模式：设置合理的约束范围
            if let Err(e) = window.set_min_size(Some(tauri::LogicalSize::new(window::MIN_WIDTH, window::MIN_HEIGHT))) {
                return Err(format!("设置最小窗口大小失败: {}", e));
            }

            if let Err(e) = window.set_max_size(Some(tauri::LogicalSize::new(window::MAX_WIDTH, window::MAX_HEIGHT))) {
                return Err(format!("设置最大窗口大小失败: {}", e));
            }

            // 设置为默认大小
            if let Err(e) = window.set_size(tauri::LogicalSize::new(size_update.width, size_update.height)) {
                return Err(format!("设置窗口大小失败: {}", e));
            }

            log::debug!("窗口已设置为自由拉伸模式，默认大小: {}x{}", size_update.width, size_update.height);
        }

        // 重新应用置顶状态，确保窗口大小变更不会影响置顶设置
        if let Err(e) = window.set_always_on_top(always_on_top) {
            log::warn!("重新应用置顶状态失败: {}", e);
        } else {
            log::debug!("置顶状态已重新应用: {}", always_on_top);
        }
    }

    Ok(())
}

/// 更新窗口位置并保存到配置
#[tauri::command]
pub async fn update_window_position(position_update: WindowPositionUpdate, state: State<'_, AppState>, app: tauri::AppHandle) -> Result<(), String> {
    // 验证位置是否有效
    if !is_position_valid(position_update.x, position_update.y) {
        return Err(format!("无效的窗口位置: ({}, {})", position_update.x, position_update.y));
    }

    // 更新配置
    {
        let mut config = state.config.lock().map_err(|e| format!("获取配置失败: {}", e))?;
        config.ui_config.window_config.position_x = Some(position_update.x);
        config.ui_config.window_config.position_y = Some(position_update.y);
    }

    // 保存配置
    save_config(&state, &app).await.map_err(|e| format!("保存配置失败: {}", e))?;

    log::debug!("窗口位置已保存: ({}, {})", position_update.x, position_update.y);

    Ok(())
}

/// 获取当前窗口位置（逻辑坐标）
#[tauri::command]
pub async fn get_current_window_position(app: tauri::AppHandle) -> Result<serde_json::Value, String> {
    if let Some(window) = app.get_webview_window("main") {
        // 检查窗口是否最小化
        if let Ok(is_minimized) = window.is_minimized() {
            if is_minimized {
                return Err("窗口已最小化，跳过位置获取".to_string());
            }
        }

        // 获取物理位置并转换为逻辑位置
        if let Ok(physical_position) = window.outer_position() {
            let scale_factor = window.scale_factor().unwrap_or(1.0);
            
            // 转换为逻辑坐标
            let logical_x = (physical_position.x as f64 / scale_factor).round() as i32;
            let logical_y = (physical_position.y as f64 / scale_factor).round() as i32;

            let position = serde_json::json!({
                "x": logical_x,
                "y": logical_y,
                "scale_factor": scale_factor
            });
            return Ok(position);
        }
    }

    Err("无法获取当前窗口位置".to_string())
}

/// 验证窗口位置是否有效
fn is_position_valid(x: i32, y: i32) -> bool {
    // 允许负值（多显示器可能有负坐标），但限制在合理范围内
    (-10000..=10000).contains(&x) && (-10000..=10000).contains(&y)
}
