mod policy;
mod reader;
mod transport;
mod ui;

use anyhow::Result;
use tao::{
    dpi::LogicalSize,
    event::{Event, WindowEvent},
    event_loop::{ControlFlow, EventLoopBuilder},
    window::WindowBuilder,
};
use tracing::{info, warn};
use ui::webview::{UiCommand, WebviewHandle};
use wry::WebViewBuilder;

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,eudi_access=debug".into()),
        )
        .init();

    info!("EUDI Door Access Terminal starting…");

    // Generate ISO 18013-5 mDL discovery QR code ahead of UI initialization
    let ble_service_uuid = uuid::Uuid::new_v4();
    let (initial_qr_uri, initial_qr_svg) = match transport::qr::generate_iso_mdl_engagement_uri(ble_service_uuid) {
        Ok(qr_uri) => {
            info!("Generated ISO 18013-5 discovery URI: {}", qr_uri);
            if let Ok(qr_ascii) = transport::qr::render_terminal_qr(&qr_uri) {
                println!(
                    "\n================ ISO 18013-5 mDL DISCOVERY QR ================\n{}\nURI: {}\n=================================================================\n",
                    qr_ascii, qr_uri
                );
            }
            let svg = transport::qr::render_svg_qr(&qr_uri).unwrap_or_default();
            (qr_uri, svg)
        }
        Err(e) => {
            warn!("Failed to generate ISO mDL discovery QR code: {e:#}");
            (String::new(), String::new())
        }
    };

    let event_loop = EventLoopBuilder::<UiCommand>::with_user_event().build();
    let proxy = event_loop.create_proxy();
    let webview_handle = WebviewHandle::new(proxy.clone());

    let window = WindowBuilder::new()
        .with_title("EUDI mDL Edge Terminal - Clean Door Access")
        .with_inner_size(LogicalSize::new(1024.0, 768.0))
        .with_min_inner_size(LogicalSize::new(680.0, 600.0))
        .build(&event_loop)?;

    let initial_html = ui::render_terminal_html(&initial_qr_uri, &initial_qr_svg);

    let proxy_ipc = proxy.clone();
    let webview = WebViewBuilder::new()
        .with_html(initial_html)
        .with_ipc_handler(move |req: wry::http::Request<String>| {
            let body = req.body().as_str();
            if body == "ready" || body == "request_qr" {
                let _ = proxy_ipc.send_event(UiCommand::ClientReady);
            }
        })
        .build(&window)?;

    let handle_clone = webview_handle.clone();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("Failed to build Tokio runtime");

        if let Err(e) = rt.block_on(run_door_controller(handle_clone)) {
            tracing::error!("Door controller loop error: {:#}", e);
        }
    });

    let mut current_state = ("standby".to_string(), None::<String>);
    let mut current_qr = (initial_qr_uri, initial_qr_svg);

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;

        match event {
            Event::UserEvent(cmd) => match cmd {
                UiCommand::ClientReady => {
                    let js_state = if let Some(ref extra) = current_state.1 {
                        format!(
                            "if (window.switchState) window.switchState({}, {});",
                            serde_json::to_string(&current_state.0).unwrap_or_default(),
                            serde_json::to_string(extra).unwrap_or_default()
                        )
                    } else {
                        format!(
                            "if (window.switchState) window.switchState({});",
                            serde_json::to_string(&current_state.0).unwrap_or_default()
                        )
                    };
                    let js_qr = format!(
                        "if (window.setIsoMdlQr) window.setIsoMdlQr({}, {});",
                        serde_json::to_string(&current_qr.0).unwrap_or_default(),
                        serde_json::to_string(&current_qr.1).unwrap_or_default()
                    );
                    let _ = webview.evaluate_script(&format!("{}; {}", js_state, js_qr));
                }
                UiCommand::SetState { state, info } => {
                    current_state = (state.clone(), info.clone());
                    let js = if let Some(extra) = info {
                        format!(
                            "if (window.switchState) window.switchState({}, {});",
                            serde_json::to_string(&state).unwrap_or_default(),
                            serde_json::to_string(&extra).unwrap_or_default()
                        )
                    } else {
                        format!(
                            "if (window.switchState) window.switchState({});",
                            serde_json::to_string(&state).unwrap_or_default()
                        )
                    };
                    let _ = webview.evaluate_script(&js);
                }
                UiCommand::SetIsoMdlQr { uri, svg } => {
                    current_qr = (uri.clone(), svg.clone());
                    let js = format!(
                        "if (window.setIsoMdlQr) window.setIsoMdlQr({}, {});",
                        serde_json::to_string(&uri).unwrap_or_default(),
                        serde_json::to_string(&svg).unwrap_or_default()
                    );
                    let _ = webview.evaluate_script(&js);
                }
                UiCommand::Close => *control_flow = ControlFlow::Exit,
            },
            Event::WindowEvent {
                event: WindowEvent::Resized(size),
                ..
            } => {
                let _ = webview.set_bounds(wry::Rect {
                    position: wry::dpi::Position::Logical(wry::dpi::LogicalPosition::new(0.0, 0.0)),
                    size: wry::dpi::Size::Physical(size),
                });
            }
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } => {
                *control_flow = ControlFlow::Exit;
            }
            _ => {}
        }
    });
}

async fn run_door_controller(ui_handle: WebviewHandle) -> Result<()> {
    // Trusted IACA roots (issuer CA certificates) — load from disk
    let trust_anchors = reader::load_trust_anchors("trust/iaca/")?;

    // Set initial UI state: Discovery (ready for mobile device tap)
    ui_handle.set_discovery();

    loop {
        // Poll for NFC tap from holder
        match transport::wait_for_engagement().await {
            Ok(engagement) => {
                info!("Got DeviceEngagement from NFC ({} bytes)", engagement.bytes.len());

                // State 2: Connecting (device connecting over BLE)
                ui_handle.set_connecting();

                match reader::run_session(engagement, &trust_anchors).await {
                    Ok(resp) => {
                        if policy::should_unlock(&resp) {
                            info!("ACCESS GRANTED");
                            let resource = resp
                                .elements
                                .get("granted_resource")
                                .and_then(|v| v.as_str())
                                .unwrap_or("dc:augsburg:maximilianviertel:backup")
                                .to_string();

                            // State 3: Access Granted (validation was successful)
                            ui_handle.set_access_granted(resource);

                            // Hold unlock state for 5 seconds (handled by JS timer in UI)
                            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                        } else {
                            info!("ACCESS DENIED by policy");
                            // State 4: Access Denied (validation was unsuccessful)
                            ui_handle.set_access_denied(
                                "Dieses Wallet besitzt keine Freigabe für Tür IAA-GATE-04.".to_string(),
                            );
                            tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                        }
                    }
                    Err(e) => {
                        warn!("ISO 18013-5 session failed: {e:#}");
                        // State 4: Access Denied
                        ui_handle.set_access_denied(format!("Übertragungsfehler: {e:#}"));
                        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                    }
                }

                // Return to State 1: Discovery
                ui_handle.set_discovery();
            }
            Err(e) => {
                warn!("NFC reader unavailable or polling error ({e:#}). Waiting before retry...");
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            }
        }
    }
}