//! Platform adapter access, isolated behind a small injectable discovery interface.
use bikebridge_core::{AdapterState, BridgeError, ErrorCode, Result};
use btleplug::{
    api::{Central, CentralState, Manager as _, Peripheral as _, ScanFilter},
    platform::{Adapter, Manager, Peripheral},
};
use std::{
    collections::{HashMap, HashSet},
    future::Future,
    sync::Arc,
};
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
    /// Click V2 model and side from Zwift manufacturer data; not a verified input capability.
    pub click_v2: Option<crate::click::Side>,
    /// Explicitly selected by the user by name; permits discovery before services are known.
    /// This does not verify a device role or grant any control capability.
    pub manually_selected: bool,
    /// Private peripheral identity, scoped to its adapter.
    pub key: String,
    /// OS-cached or advertised name, if available.
    pub name: Option<String>,
    /// Received signal strength in dBm.
    pub rssi: Option<i16>,
    /// Advertised service UUIDs, including service-data keys.
    pub services: Vec<Uuid>,
}

/// Discovery backend. Implementations may be injected for hardware-free tests.
/// Discovery never connects automatically; a retained transport handles explicit connections.
pub trait DiscoveryBackend: Send + 'static {
    /// Include exactly one peripheral selected from nearby results, without connecting.
    fn select_device(&mut self, _adapter: &str, _key: &str) -> Result<()> {
        Err(BridgeError::new(
            ErrorCode::UnsupportedOperation,
            "This backend does not support nearby device selection.",
        ))
    }
    /// Include a user-selected name in subsequent scans without connecting it.
    fn select_name(&mut self, _name: String) -> Result<()> {
        Err(BridgeError::new(
            ErrorCode::UnsupportedOperation,
            "This discovery backend does not support name selection.",
        ))
    }
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
    /// Resolve a verified OpenBikeControl candidate without connecting.
    fn controller(
        &self,
        _adapter: &str,
        _key: &str,
    ) -> Option<Arc<dyn bikebridge_openbikecontrol::ControllerTransport>> {
        None
    }
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
    selected_devices: HashSet<(String, String)>,
    device_names: Vec<String>,
    manager: Option<Manager>,
    adapters: HashMap<String, Adapter>,
    // Retain opaque platform handles for future connection support, not public addresses.
    peripherals: HashMap<(String, String), Peripheral>,
    click_sides: HashMap<(String, String), crate::click::Side>,
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
    /// Include explicitly named peripherals even when they advertise no cycling services.
    /// Unrelated devices are still excluded by the registry.
    pub fn with_device_names(device_names: Vec<String>) -> Self {
        Self {
            device_names,
            ..Self::default()
        }
    }

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
    fn select_device(&mut self, adapter: &str, key: &str) -> Result<()> {
        self.selected_devices
            .insert((adapter.to_owned(), key.to_owned()));
        Ok(())
    }
    fn select_name(&mut self, name: String) -> Result<()> {
        if !self
            .device_names
            .iter()
            .any(|existing| device_name_matches(existing, &name))
        {
            self.device_names.push(name);
        }
        Ok(())
    }
    fn controller(
        &self,
        adapter: &str,
        key: &str,
    ) -> Option<Arc<dyn bikebridge_openbikecontrol::ControllerTransport>> {
        self.peripherals
            .get(&(adapter.to_owned(), key.to_owned()))
            .cloned()
            .map(|peripheral| {
                if let Some(&side) = self.click_sides.get(&(adapter.to_owned(), key.to_owned())) {
                    Arc::new(crate::click_transport::NativeClick { peripheral, side })
                        as Arc<dyn bikebridge_openbikecontrol::ControllerTransport>
                } else {
                    Arc::new(crate::controller_transport::NativeController(peripheral))
                        as Arc<dyn bikebridge_openbikecontrol::ControllerTransport>
                }
            })
    }
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
            // Click models are identified by manufacturer data, which may arrive without
            // service UUIDs. Post-filter snapshots instead of excluding those advertisements.
            .start_scan(ScanFilter::default())
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
            let click_v2 = properties
                .manufacturer_data
                .iter()
                .find_map(|(&company, payload)| {
                    crate::click::Side::from_manufacturer(company, payload)
                })
                .or_else(|| {
                    self.click_sides
                        .get(&(key.to_owned(), peripheral_key.clone()))
                        .copied()
                });
            let name = properties.local_name.or(properties.advertisement_name);
            let manually_selected = self
                .selected_devices
                .contains(&(key.to_owned(), peripheral_key.clone()))
                || name.as_ref().is_some_and(|name| {
                    self.device_names
                        .iter()
                        .any(|selected| device_name_matches(selected, name))
                });
            let mut services = properties.services;
            services.extend(properties.service_data.into_keys());
            // Retain handles only for cycling devices and explicit name selections. Partial later advertisements
            // still get merged by the registry if the device was already identified.
            if (click_v2.is_some()
                || manually_selected
                || crate::classification::classify(&services).is_some())
                && (self.peripherals.len() < 1024
                    || self
                        .peripherals
                        .contains_key(&(key.to_owned(), peripheral_key.clone())))
            {
                if let Some(side) = click_v2 {
                    self.click_sides
                        .insert((key.to_owned(), peripheral_key.clone()), side);
                }
                self.peripherals
                    .insert((key.to_owned(), peripheral_key.clone()), peripheral);
            }
            advertisements.push(Advertisement {
                click_v2,
                manually_selected,
                key: peripheral_key,
                name,
                rssi: properties.rssi,
                services,
            });
        }
        Ok(advertisements)
    }
}

/// Match a whole display name, ignoring case, surrounding space, and curly apostrophes.
/// Names identify candidates only; GATT services must be verified after connecting.
pub fn device_name_matches(selected: &str, actual: &str) -> bool {
    let normalize = |name: &str| {
        name.trim()
            .replace(['\u{2018}', '\u{2019}'], "'")
            .to_lowercase()
    };
    !selected.trim().is_empty() && normalize(selected) == normalize(actual)
}
