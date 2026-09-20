//! The desktop owns one embedded server, or borrows an existing compatible daemon.
//! A borrowed daemon is never stopped by this app. No child process is required.
use bikebridge_ble::{NativeBackend, Scanner};
use bikebridge_core::SafetyLimits;
use bikebridge_server::AppState;
use std::{
    io::ErrorKind,
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};
use tokio::{net::TcpListener, sync::Mutex, task::JoinHandle};
use tokio_util::sync::CancellationToken;

pub const PORT: u16 = 9376;

#[derive(Default)]
pub struct Runtime {
    owned: Mutex<Option<Running>>,
    closing: AtomicBool,
}

struct Running {
    stop: CancellationToken,
    task: JoinHandle<std::io::Result<()>>,
}

async fn compatible(port: u16) -> bool {
    let Ok(client) = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(2))
        .build()
    else {
        return false;
    };
    let Ok(response) = client
        .get(format!("http://127.0.0.1:{port}/api/status"))
        .send()
        .await
    else {
        return false;
    };
    if !response.status().is_success() {
        return false;
    }
    let Ok(status) = response.json::<serde_json::Value>().await else {
        return false;
    };
    status["protocolVersion"] == 1
        && status["version"].is_string()
        && status["bluetoothEnabled"].is_boolean()
        && status["mockMode"].is_boolean()
        && status["scan"].is_object()
}

impl Runtime {
    /// Returns true when this app owns the service. Repeated calls are serialized.
    pub async fn ensure(&self) -> Result<bool, String> {
        self.ensure_at(PORT, false).await
    }

    async fn ensure_at(&self, port: u16, mock: bool) -> Result<bool, String> {
        let mut owned = self.owned.lock().await;
        if self.closing.load(Ordering::SeqCst) {
            return Err("BikeBridge is shutting down.".into());
        }
        if owned
            .as_ref()
            .is_some_and(|server| !server.task.is_finished())
        {
            return Ok(true);
        }
        if let Some(previous) = owned.take() {
            previous.stop.cancel();
            let _ = previous.task.await;
        }
        // Bind before touching Bluetooth. Two starting clients cannot both own the adapter.
        let listener = match TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port)).await {
            Ok(listener) => listener,
            Err(error) if error.kind() == ErrorKind::AddrInUse => {
                // An existing daemon may still be starting. Never pick a random port: OBS URLs must stay stable.
                for _ in 0..5 {
                    if compatible(port).await {
                        return Ok(false);
                    }
                    tokio::time::sleep(Duration::from_millis(200)).await;
                }
                return Err(format!(
                    "Port {port} is in use, but a compatible BikeBridge service did not respond. Close the other app using that port, then choose Retry."
                ));
            }
            Err(error) => return Err(format!("Could not start BikeBridge: {error}")),
        };
        let mut state = AppState::new(mock, SafetyLimits::default()).map_err(|e| e.to_string())?;
        if !mock {
            let scanner =
                Scanner::spawn(NativeBackend::default(), state.events.clone(), None).await;
            // Scan failures remain visible in the dashboard and can be retried there.
            let _ = scanner.start().await;
            state = state.with_scanner(scanner);
        }
        let stop = CancellationToken::new();
        let signal = stop.clone();
        let task = tokio::spawn(bikebridge_server::serve(listener, state, signal));
        *owned = Some(Running { stop, task });
        Ok(true)
    }

    pub async fn shutdown(&self) {
        self.closing.store(true, Ordering::SeqCst);
        if let Some(server) = self.owned.lock().await.take() {
            server.stop.cancel();
            // Existing server cleanup resets trainer control and disconnects owned devices.
            let _ = server.task.await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn embedded_service_serves_products_and_borrowers_do_not_stop_it() {
        let reserved = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = reserved.local_addr().unwrap().port();
        drop(reserved);
        let owner = Runtime::default();
        assert!(owner.ensure_at(port, true).await.unwrap());
        assert!(owner.ensure_at(port, true).await.unwrap());
        let borrower = Runtime::default();
        assert!(!borrower.ensure_at(port, true).await.unwrap());
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        for path in ["/", "/overlay/", "/overlay/app.js", "/api/devices"] {
            let response = client
                .get(format!("http://127.0.0.1:{port}{path}"))
                .send()
                .await
                .unwrap();
            assert!(response.status().is_success(), "{path}");
            assert!(!response.bytes().await.unwrap().is_empty());
        }
        borrower.shutdown().await;
        assert!(compatible(port).await);
        owner.shutdown().await;
        assert!(
            TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port))
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn unrelated_listener_is_not_reused_or_stopped() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let runtime = Runtime::default();
        assert!(
            runtime
                .ensure_at(port, true)
                .await
                .unwrap_err()
                .contains("in use")
        );
        runtime.shutdown().await;
        assert!(
            TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port))
                .await
                .is_err()
        );
    }
}
