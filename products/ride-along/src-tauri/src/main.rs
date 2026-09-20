#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
mod runtime;

use std::{
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tauri::Manager;
use tauri::{State, ipc::Channel};
use tauri_plugin_opener::OpenerExt;
use tokio_tungstenite::tungstenite::Message;

#[derive(Default)]
struct Bridge {
    stream: Mutex<Option<tauri::async_runtime::JoinHandle<()>>>,
}

fn base(port: u16) -> Result<String, String> {
    if port == 0 {
        return Err("Enter a port from 1 to 65535.".into());
    }
    Ok(format!("http://127.0.0.1:{port}"))
}
fn device_path(id: &str) -> Result<String, String> {
    if id.is_empty()
        || id.len() > 128
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err("Invalid device identity.".into());
    }
    Ok(format!("/api/devices/{id}/connect"))
}
async fn request(port: u16, path: &str, post: bool) -> Result<Value, String> {
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|e| e.to_string())?;
    let url = format!("{}{path}", base(port)?);
    let response = if post {
        client.post(url)
    } else {
        client.get(url)
    }
    .send()
    .await
    .map_err(|_| {
        format!("BikeBridge is unavailable on port {port}. Open Settings and choose Retry.")
    })?;
    let status = response.status();
    let value: Value = response
        .json()
        .await
        .map_err(|_| "BikeBridge returned an invalid response.".to_owned())?;
    if !status.is_success() {
        return Err(value["data"]["message"]
            .as_str()
            .unwrap_or("BikeBridge request failed.")
            .to_owned());
    }
    Ok(value)
}

#[tauri::command]
async fn bridge_snapshot(port: u16) -> Result<Value, String> {
    let (status, devices) = tokio::try_join!(
        request(port, "/api/status", false),
        request(port, "/api/devices", false)
    )?;
    Ok(json!({"status":status,"devices":devices}))
}
#[tauri::command]
async fn bridge_connect_device(port: u16, id: String) -> Result<Value, String> {
    request(port, &device_path(&id)?, true).await
}
#[tauri::command]
fn bridge_stop(state: State<'_, Bridge>) -> Result<(), String> {
    if let Some(task) = state
        .stream
        .lock()
        .map_err(|_| "Bridge state unavailable.")?
        .take()
    {
        task.abort();
    }
    Ok(())
}
#[tauri::command]
fn bridge_subscribe(
    port: u16,
    channel: Channel<Value>,
    state: State<'_, Bridge>,
) -> Result<(), String> {
    base(port)?;
    let mut current = state
        .stream
        .lock()
        .map_err(|_| "Bridge state unavailable.")?;
    if let Some(task) = current.take() {
        task.abort();
    }
    *current = Some(tauri::async_runtime::spawn(async move {
        let mut delay = 1;
        loop {
            if channel
                .send(json!({"type":"bridge.connection","data":"connecting"}))
                .is_err()
            {
                return;
            }
            let connected = tokio::time::timeout(
                Duration::from_secs(5),
                tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}/ws")),
            )
            .await;
            if let Ok(Ok((mut socket, _))) = connected {
                loop {
                    let incoming =
                        tokio::time::timeout(Duration::from_secs(35), socket.next()).await;
                    match incoming {
                        Ok(Some(Ok(Message::Text(text)))) => {
                            let Ok(value) = serde_json::from_str::<Value>(&text) else {
                                break;
                            };
                            if value["type"] == "hello" {
                                if value["protocolVersion"] != 1 {
                                    break;
                                }
                                let subscription = json!({"type":"subscribe","requestId":"ride-along","events":["telemetry","device","error","replay"]});
                                if socket
                                    .send(Message::Text(subscription.to_string().into()))
                                    .await
                                    .is_err()
                                {
                                    break;
                                }
                            }
                            if value["type"] == "response" && value["requestId"] == "ride-along" {
                                if value["success"] != true {
                                    break;
                                }
                                delay = 1;
                                if channel
                                    .send(json!({"type":"bridge.connection","data":"online"}))
                                    .is_err()
                                {
                                    return;
                                }
                            }
                            if channel.send(value).is_err() {
                                return;
                            }
                        }
                        Ok(Some(Ok(Message::Ping(bytes)))) => {
                            if socket.send(Message::Pong(bytes)).await.is_err() {
                                break;
                            }
                        }
                        Ok(Some(Ok(Message::Pong(_)))) => {}
                        _ => break,
                    }
                }
            }
            if channel
                .send(json!({"type":"bridge.connection","data":"offline"}))
                .is_err()
            {
                return;
            }
            tokio::time::sleep(Duration::from_secs(delay)).await;
            delay = (delay * 2).min(10);
        }
    }));
    Ok(())
}

#[derive(Default)]
struct Quitting(AtomicBool);

#[tauri::command]
async fn bridge_ensure_service(state: State<'_, runtime::Runtime>) -> Result<bool, String> {
    state.ensure().await
}

#[tauri::command]
fn bridge_open_product(app: tauri::AppHandle, port: u16, product: String) -> Result<(), String> {
    let path = match product.as_str() {
        "dashboard" => "/",
        "overlay" => "/overlay/",
        _ => return Err("Unknown BikeBridge product.".into()),
    };
    app.opener()
        .open_url(format!("{}{path}", base(port)?), None::<&str>)
        .map_err(|e| e.to_string())
}

fn show_ride(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

fn quit(app: &tauri::AppHandle) {
    if app.state::<Quitting>().0.swap(true, Ordering::SeqCst) {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        app.state::<runtime::Runtime>().shutdown().await;
        app.exit(0);
    });
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _, _| {
            show_ride(app)
        }))
        .plugin(tauri_plugin_opener::init())
        .manage(Bridge::default())
        .manage(runtime::Runtime::default())
        .manage(Quitting::default())
        .setup(|app| {
            use tauri::{
                menu::{Menu, MenuItem},
                tray::TrayIconBuilder,
            };
            let ride = MenuItem::with_id(app, "ride", "Ride Along", true, None::<&str>)?;
            let dashboard =
                MenuItem::with_id(app, "dashboard", "Devices & dashboard", true, None::<&str>)?;
            let overlay = MenuItem::with_id(app, "overlay", "Stream overlay", true, None::<&str>)?;
            let exit = MenuItem::with_id(app, "quit", "Quit BikeBridge", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&ride, &dashboard, &overlay, &exit])?;
            TrayIconBuilder::with_id("bikebridge")
                .icon(app.default_window_icon().ok_or("Missing app icon")?.clone())
                .tooltip("BikeBridge — cycling apps")
                .menu(&menu)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "ride" => show_ride(app),
                    "dashboard" | "overlay" => {
                        if let Err(error) = bridge_open_product(
                            app.clone(),
                            runtime::PORT,
                            event.id.as_ref().to_owned(),
                        ) {
                            eprintln!("{error}");
                            show_ride(app);
                        }
                    }
                    "quit" => quit(app),
                    _ => {}
                })
                .build(app)?;
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                // The UI also awaits ensure(), receiving any startup error with a Retry action.
                let _ = handle.state::<runtime::Runtime>().ensure().await;
            });
            #[cfg(target_os = "macos")]
            {
                use objc2_app_kit::{NSWindow, NSWindowCollectionBehavior as Behavior};
                let window = app
                    .get_webview_window("main")
                    .ok_or("Missing main window")?;
                // Tauri setup runs on the main thread; the live webview owns this NSWindow.
                let native = unsafe { &*(window.ns_window()? as *const NSWindow) };
                let mut behavior = native.collectionBehavior();
                behavior.remove(Behavior::FullScreenPrimary | Behavior::FullScreenNone);
                behavior.insert(Behavior::CanJoinAllSpaces | Behavior::FullScreenAuxiliary);
                native.setCollectionBehavior(behavior);
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .invoke_handler(tauri::generate_handler![
            bridge_snapshot,
            bridge_connect_device,
            bridge_subscribe,
            bridge_stop,
            bridge_ensure_service,
            bridge_open_product
        ])
        .build(tauri::generate_context!())
        .expect("Unable to start BikeBridge")
        .run(|app, event| match event {
            tauri::RunEvent::ExitRequested { api, code, .. } if code != Some(0) => {
                api.prevent_exit();
                quit(app);
            }
            #[cfg(target_os = "macos")]
            tauri::RunEvent::Reopen { .. } => show_ride(app),
            _ => {}
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_requests_stay_on_loopback_and_device_paths_cannot_escape() {
        assert!(base(0).is_err());
        assert_eq!(base(9380).unwrap(), "http://127.0.0.1:9380");
        assert_eq!(
            device_path("ble-abc123").unwrap(),
            "/api/devices/ble-abc123/connect"
        );
        for id in ["", "../status", "x?url=example.com", "x/y", "x\\y", "%2f"] {
            assert!(device_path(id).is_err());
        }
    }
}
