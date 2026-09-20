use crate::{backend::Advertisement, classification::classify};
use bikebridge_core::{DeviceInfo, Event};
use std::collections::{HashMap, HashSet};
use uuid::Uuid;

#[derive(Default)]
pub(crate) struct Registry {
    devices: HashMap<(String, String), Entry>,
}
struct Entry {
    info: DeviceInfo,
    services: HashSet<Uuid>,
}

impl Registry {
    pub fn private_keys(&self, id: &str) -> Option<(&str, &str)> {
        self.devices
            .iter()
            .find(|(_, entry)| entry.info.id == id)
            .map(|((adapter, key), _)| (adapter.as_str(), key.as_str()))
    }
    pub fn devices(&self) -> Vec<DeviceInfo> {
        let mut devices: Vec<_> = self
            .devices
            .values()
            .map(|entry| entry.info.clone())
            .collect();
        devices.sort_by(|a, b| a.id.cmp(&b.id));
        devices
    }

    pub fn observe(&mut self, adapter: &str, advertisement: Advertisement) -> Option<Event> {
        let key = (adapter.to_owned(), advertisement.key);
        let is_new = !self.devices.contains_key(&key);
        if is_new && classify(&advertisement.services).is_none() {
            return None;
        }
        // Bound our own discovery registry; btleplug maintains its platform cache separately.
        if is_new && self.devices.len() >= 1024 {
            return None;
        }
        let entry = self.devices.entry(key).or_insert_with(|| Entry {
            info: DeviceInfo {
                id: format!("ble-{}", Uuid::new_v4()),
                name: "Unnamed cycling device".into(),
                kind: bikebridge_core::DeviceKind::Unknown,
                transport: "bluetooth".into(),
                connected: false,
                signal_strength: None,
                capabilities: Vec::new(),
            },
            services: HashSet::new(),
        });
        let old = entry.info.clone();
        entry.services.extend(
            advertisement
                .services
                .into_iter()
                .filter(|uuid| classify(&[*uuid]).is_some()),
        );
        if let Some(kind) = classify(&entry.services.iter().copied().collect::<Vec<_>>()) {
            entry.info.kind = kind;
        }
        if let Some(name) = advertisement.name.filter(|name| !name.trim().is_empty()) {
            entry.info.name = name.chars().filter(|c| !c.is_control()).take(128).collect();
        }
        if let Some(rssi) = advertisement.rssi {
            entry.info.signal_strength = Some(rssi);
        }
        // An advertisement cannot confirm GATT features, measurement fields, or controllability.
        if is_new {
            Some(Event::DeviceDiscovered {
                data: entry.info.clone(),
            })
        } else if old != entry.info {
            Some(Event::DeviceUpdated {
                data: entry.info.clone(),
            })
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::classification::*;
    fn ad(services: Vec<Uuid>) -> Advertisement {
        Advertisement {
            key: "AA:BB:CC:DD:EE:FF".into(),
            name: None,
            rssi: Some(-50),
            services,
        }
    }
    #[test]
    fn identity_deduplication_updates_and_partial_advertisements() {
        let mut registry = Registry::default();
        assert!(registry.observe("os-adapter", ad(vec![])).is_none());
        let event = registry
            .observe("os-adapter", ad(vec![HEART_RATE]))
            .expect("discovered");
        assert!(matches!(event, Event::DeviceDiscovered { .. }));
        let id = registry.devices()[0].id.clone();
        assert!(
            !serde_json::to_string(&event)
                .expect("JSON")
                .contains("AA:BB")
        );
        assert!(
            registry
                .observe("os-adapter", ad(vec![HEART_RATE]))
                .is_none()
        );
        assert!(registry.observe("os-adapter", ad(vec![])).is_none());
        let mut updated = ad(vec![FITNESS_MACHINE]);
        updated.name = Some("Trainer".into());
        assert!(matches!(
            registry.observe("os-adapter", updated),
            Some(Event::DeviceUpdated { .. })
        ));
        let info = &registry.devices()[0];
        assert_eq!(info.id, id);
        assert_eq!(info.kind, bikebridge_core::DeviceKind::Trainer);
        assert!(info.capabilities.is_empty());
        assert!(!info.connected);
        registry.observe("other-adapter", ad(vec![FITNESS_MACHINE]));
        assert_eq!(registry.devices().len(), 2);
        assert_ne!(registry.devices()[0].id, registry.devices()[1].id);
    }
}
