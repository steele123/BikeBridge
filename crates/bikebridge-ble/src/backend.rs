//! Platform adapter access, isolated behind a small injectable discovery interface.
use crate::classification::cycling_services;
use bikebridge_core::{AdapterState, BridgeError, ErrorCode, Result};
use btleplug::{
    api::{Central, CentralState, Manager as _, Peripheral as _, ScanFilter},
    platform::{Adapter, Manager, Peripheral},
};
use std::{collections::HashMap, future::Future, sync::Arc};
use uuid::Uuid;

/// Internal adapter description. `key` is transport-private and must never be serialized.
#[derive(Clone)]
pub struct BackendAdapter {
    /// Stable within the backend; may contain an OS identifier.
    pub key: String,
    /// Radio state.
    pub state: AdapterState,
}

/// Snapshot of advertisement properties; does not expose raw manufacturer payloads.
#[derive(Clone)]
pub struct Advertisement {
    /// Private peripheral identity, scoped to its adapter.
    pub key: String,
    /// Advertised local name, if available.
    pub name: Option<String>,
    /// Received signal strength in dBm.
    pub rssi: Option<i16>,
    /// Advertised service UUIDs, including service-data keys.
    pub services: Vec<Uuid>,
}

/// Discovery backend. Implementations may be injected for hardware-free tests.
/// Discovery never connects automatically; a retained transport handles explicit connections.
pub trait DiscoveryBackend: Send + 'static {
    /// Enumerate radios. Preserve handles for an adapter already scanning.
    fn adapters(&mut self) -> impl Future<Output = Result<Vec<BackendAdapter>>> + Send;
    /// Start a scan on the selected private adapter key.
    fn start_scan(&mut self, adapter: &str) -> impl Future<Output = Result<()>> + Send;
    /// Stop scanning, including after a partially failed start.
    fn stop_scan(&mut self, adapter: &str) -> impl Future<Output = Result<()>> + Send;
    /// Read current advertisement snapshots; report unavailable radios as errors.
    fn advertisements(
        &mut self,
        adapter: &str,
    ) -> impl Future<Output = Result<Vec<Advertisement>>> + Send;
    /// Resolve a retained peripheral without starting any I/O. Discovery-only backends may omit this.
    fn peripheral(
        &self,
        _adapter: &str,
        _key: &str,
    ) -> Option<Arc<dyn crate::transport::FtmsTransport>> {
        None
    }
}

/// Native btleplug backend; constructed lazily so mock mode never touches Bluetooth.
#[derive(Default)]
pub struct NativeBackend {
    manager: Option<Manager>,
    adapters: HashMap<String, Adapter>,
    // Retain opaque platform handles for future connection support, not public addresses.
    peripherals: HashMap<(String, String), Peripheral>,
}

fn platform_error(error: btleplug::Error) -> BridgeError {
    tracing::debug!(%error, "Bluetooth platform operation failed");
    match error {
        btleplug::Error::PermissionDenied => BridgeError::new(
            ErrorCode::BluetoothUnavailable,
            "Bluetooth permission was denied. Check OS permissions.",
        ),
        _ => BridgeError::new(
            ErrorCode::ScanFailed,
            "Bluetooth operation failed. Check the radio and OS Bluetooth service; debug logs contain details.",
        ),
    }
}

impl NativeBackend {
    fn adapter(&self, key: &str) -> Result<Adapter> {
        self.adapters.get(key).cloned().ok_or_else(|| {
            BridgeError::new(
                ErrorCode::AdapterNotFound,
                "Bluetooth adapter is no longer available.",
            )
        })
    }
}

impl DiscoveryBackend for NativeBackend {
    fn peripheral(
        &self,
        adapter: &str,
        key: &str,
    ) -> Option<Arc<dyn crate::transport::FtmsTransport>> {
        self.peripherals
            .get(&(adapter.to_owned(), key.to_owned()))
            .cloned()
            .map(|peripheral| {
                Arc::new(crate::transport::NativeTransport(peripheral))
                    as Arc<dyn crate::transport::FtmsTransport>
            })
    }
    async fn adapters(&mut self) -> Result<Vec<BackendAdapter>> {
        if self.manager.is_none() {
            self.manager = Some(Manager::new().await.map_err(|error| {
                tracing::debug!(%error, "Bluetooth manager unavailable");
                BridgeError::new(
                    ErrorCode::BluetoothUnavailable,
                    "Bluetooth is unavailable. Check OS services and permissions.",
                )
            })?);
        }
        let manager = self.manager.as_ref().ok_or_else(|| {
            BridgeError::new(
                ErrorCode::BluetoothUnavailable,
                "Bluetooth manager unavailable.",
            )
        })?;
        let found = manager.adapters().await.map_err(platform_error)?;
        let mut descriptions = Vec::new();
        for (index, adapter) in found.into_iter().enumerate() {
            // adapter_info is just "WinRT" for every Windows radio. Use the
            // private adapter address when supplied; never expose it over the API.
            let info = adapter.adapter_info().await.map_err(platform_error)?;
            let key = match adapter.adapter_address().await.map_err(platform_error)? {
                Some(address) => format!("{info}:{address}"),
                None => format!("{info}:{index}"),
            };
            let state = match adapter.adapter_state().await.map_err(platform_error)? {
                CentralState::PoweredOn => AdapterState::PoweredOn,
                CentralState::PoweredOff => AdapterState::PoweredOff,
                CentralState::Unknown => AdapterState::Unknown,
            };
            self.adapters.entry(key.clone()).or_insert(adapter);
            descriptions.push(BackendAdapter { key, state });
        }
        Ok(descriptions)
    }

    async fn start_scan(&mut self, key: &str) -> Result<()> {
        let adapter = self.adapter(key)?;
        adapter
            .start_scan(ScanFilter {
                services: cycling_services(),
            })
            .await
            .map_err(platform_error)
    }

    async fn stop_scan(&mut self, key: &str) -> Result<()> {
        self.adapter(key)?.stop_scan().await.map_err(platform_error)
    }

    async fn advertisements(&mut self, key: &str) -> Result<Vec<Advertisement>> {
        let adapter = self.adapter(key)?;
        if adapter.adapter_state().await.map_err(platform_error)? == CentralState::PoweredOff {
            return Err(BridgeError::new(
                ErrorCode::BluetoothUnavailable,
                "Bluetooth radio is powered off.",
            ));
        }
        let mut advertisements = Vec::new();
        for peripheral in adapter.peripherals().await.map_err(platform_error)? {
            let properties = match peripheral.properties().await {
                Ok(Some(properties)) => properties,
                Ok(None) => continue,
                Err(error) => {
                    tracing::debug!(%error, "Advertisement properties unavailable");
                    continue;
                }
            };
            let peripheral_key = peripheral.id().to_string();
            let mut services = properties.services;
            services.extend(properties.service_data.into_keys());
            // Retain handles only for cycling devices. Partial later advertisements
            // still get merged by the registry if the device was already identified.
            if crate::classification::classify(&services).is_some() && self.peripherals.len() < 1024
            {
                self.peripherals
                    .insert((key.to_owned(), peripheral_key.clone()), peripheral);
            }
            advertisements.push(Advertisement {
                key: peripheral_key,
                name: properties.local_name.or(properties.advertisement_name),
                rssi: properties.rssi,
                services,
            });
        }
        Ok(advertisements)
    }
}
